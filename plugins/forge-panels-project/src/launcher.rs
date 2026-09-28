//! The **Launcher** (`forge.launcher`, Ch.21 §21.21 "Launcher and new project", Ch.33 §33.5,
//! DoD M2-47): recent projects, open by location, and the **three-click** new project —
//! *New project…*, a template (2D, 3D; O-14) or a preset a
//! plugin provides (the editor's real `Preset` registry, WP-21: a preset plugin dropped in
//! while the editor runs joins the list), *Create*.
//! The name and folder are filled in (a local folder under the projects folder, the
//! default store, O-11): nothing to configure before the editor is usable, and the remote
//! is offered at the first save, not here.
//!
//! Creating and opening are **commands** (`forge.project.create`, `forge.project.open`)
//! the core performs (E-36); the launcher writes no file and holds no store (I7, I17).
//! Unsaved changes in the open project are never dropped silently: the launcher asks
//! (save first, discard, or cancel) before it sends anything. The recent list is user
//! config, updated when a create or open succeeds.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use forge_editor::panels::PanelCx;
use forge_editor::project::recent::RecentEntry;
use forge_editor::project::{
    CLONE_CMD, CREATE_CMD, OPEN_CMD, ProjectOp, Template, default_location, save_command,
};
use forge_ui::widgets::{
    EmptyState, LabelKind, Pressed, RowActivated, RowItem, SelectionChanged, TextField, VirtualTree,
};
use forge_ui::{NodeStyle, Role};

use crate::ui::{bar, button, group, set, show, text};

/// The name a new project gets unless the user types another.
pub const DEFAULT_NAME: &str = forge_ui::tr_key!("New Project");

/// `file:<dir>/<ident(name)>`, or `…_2`, `…_3` when a Forge project is already there (a
/// read-only look: the create command still refuses an existing project, PROJECT-0002).
pub fn unique_location(dir: &Path, name: &str) -> String {
    let base = default_location(dir, name);
    let taken = |loc: &str| {
        loc.strip_prefix("file:")
            .is_some_and(|p| Path::new(p).join("forge-project.ron").exists())
    };
    if !taken(&base) {
        return base;
    }
    (2..1000)
        .map(|n| format!("{base}_{n}"))
        .find(|l| !taken(l))
        .unwrap_or(base)
}

#[derive(Default)]
struct State {
    template: Option<Template>,
    /// What the template list shows, by row key: the built-in templates, then every preset
    /// a plugin adds.
    templates: Vec<Template>,
    /// An operation waiting for the unsaved-changes answer.
    pending: Option<ProjectOp>,
    recent: Vec<RecentEntry>,
}

fn recent_label(e: &RecentEntry) -> String {
    format!("{} \u{2014} {}", e.name, e.location)
}

/// The template list: the built-in templates (O-14), then each preset of the editor's
/// `Preset` registry beyond the built-in keys (a preset plugin's), with the row labels and
/// the catalog revision they were read at.
fn template_rows(
    services: &forge_editor::services::EditorServices,
) -> (Vec<Template>, Vec<String>, u64) {
    let hosting = services.hosting.borrow();
    let catalog = hosting
        .presets()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut listed: Vec<Template> = Template::ALL.to_vec();
    let mut rows: Vec<String> = Template::ALL
        .iter()
        .map(|t| format!("{} \u{2014} {}", t.label(), t.blurb()))
        .collect();
    for e in catalog
        .added()
        .filter(|_| !services.faults.launcher_builtin_templates_only())
    {
        listed.push(Template::Preset(e.key.clone()));
        rows.push(forge_ui::trf!(
            "{label} \u{2014} a {family} preset from {owner}",
            label = e.preset.workspace.label,
            owner = e.owner,
            family = match e.family_dir() {
                "2d" => "2D",
                _ => "3D",
            }
        ));
    }
    (listed, rows, catalog.revision())
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let root = pb.b.add(
            pb.parent,
            "content",
            NodeStyle::column(space[1])
                .padding(space[2])
                .grow(1.0)
                .scrollable(),
            forge_ui::widgets::Container::new(Role::Group).labelled(forge_ui::tr!("Launcher")),
        )?;
        let state = Rc::new(RefCell::new(State {
            template: Some(Template::ThreeD),
            ..State::default()
        }));

        // ---- recent projects ----
        let recent_g = group(pb, root, "recent_group", forge_ui::tr!("Recent projects"))?;
        text(pb, recent_g, "recent_title", forge_ui::tr!("Recent projects"), LabelKind::Heading)?;
        let empty = pb.b.add(
            recent_g,
            "recent_empty",
            NodeStyle::leaf(),
            EmptyState::new(
                forge_ui::tr!("No recent projects yet. Create one below, or open a project folder by its location."),
            ),
        )?;
        let recent =
            pb.b.add(
                recent_g,
                "recent",
                NodeStyle::leaf().height(160.0),
                VirtualTree::list(forge_ui::tr!("Recent projects")).single_select(),
            )?;
        let rbar = bar(pb, recent_g, "recent_bar", forge_ui::tr!("Recent project actions"))?;
        let open_recent = button(pb, rbar, "open_recent", forge_ui::tr!("Open"))?;
        let forget = button(pb, rbar, "forget", forge_ui::tr!("Remove from list"))?;

        // ---- open by location ----
        let obar = bar(pb, root, "open_bar", forge_ui::tr!("Open a project by location"))?;
        let open_loc = pb.b.signal(String::new());
        let open_field = pb.b.add(
            obar,
            "open_location",
            NodeStyle::leaf().width(360.0),
            TextField::new(open_loc, forge_ui::tr!("Project location")).placeholder(forge_ui::tr!("file:C:\\Projects\\My Game")),
        )?;
        let open = button(pb, obar, "open", forge_ui::tr!("Open"))?;

        // ---- clone from a remote (Ch.33 §33.5: paste a URL) ----
        let cbar = bar(pb, root, "clone_bar", forge_ui::tr!("Clone a project from a remote"))?;
        let clone_url = pb.b.signal(String::new());
        pb.b.add(
            cbar,
            "clone_url",
            NodeStyle::leaf().width(360.0),
            TextField::new(clone_url, forge_ui::tr!("Remote to clone"))
                .placeholder(forge_ui::tr!("https://github.com/you/my-game.git, or a folder")),
        )?;
        let clone = button(pb, cbar, "clone", forge_ui::tr!("Clone"))?;

        // ---- new project (three clicks) ----
        let new = pb.b.add(
            root,
            "new",
            NodeStyle::leaf(),
            forge_ui::widgets::Button::new(forge_ui::tr!("New project\u{2026}")).primary(),
        )?;
        let form = group(pb, root, "form", forge_ui::tr!("New project"))?;
        text(pb, form, "form_title", forge_ui::tr!("New project"), LabelKind::Heading)?;
        text(
            pb,
            form,
            "form_hint",
            forge_ui::tr!("Pick a template and press Create. A preset only sets defaults: any project can be promoted to another later."),
            LabelKind::Muted,
        )?;
        let (listed, rows, catalog_rev) = template_rows(&pb.services());
        state.borrow_mut().templates = listed;
        let mut t = VirtualTree::list(forge_ui::tr!("Templates")).single_select();
        for (i, label) in rows.into_iter().enumerate() {
            t.push(None, i as u64, RowItem::new(label));
        }
        t.select(&[1]);
        let templates =
            pb.b.add(form, "templates", NodeStyle::leaf().height(120.0), t)?;
        let name = pb.b.signal(forge_ui::l10n::tr(DEFAULT_NAME).to_string());
        pb.b.add(
            form,
            "name",
            NodeStyle::leaf().width(360.0),
            TextField::new(name, forge_ui::tr!("Project name")),
        )?;
        let location = pb.b.signal(String::new());
        pb.b.add(
            form,
            "location",
            NodeStyle::leaf().width(360.0),
            TextField::new(location, forge_ui::tr!("Project folder"))
                .placeholder(forge_ui::tr!("Leave empty: a new folder under your projects folder")),
        )?;
        let where_ = pb.b.signal(String::new());
        text(pb, form, "where", where_, LabelKind::Muted)?;
        text(
            pb,
            form,
            "store",
            forge_ui::tr!("Stored in a local folder. When you first save you can link a remote: GitHub (sign in, and the repository is created for you), any Git host, a bare Git repository on a share, or a folder."),
            LabelKind::Muted,
        )?;
        let fbar = bar(pb, form, "form_bar", forge_ui::tr!("New project actions"))?;
        let create = pb.b.add(
            fbar,
            "create",
            NodeStyle::leaf(),
            forge_ui::widgets::Button::new(forge_ui::tr!("Create")).primary(),
        )?;
        let cancel_new = button(pb, fbar, "cancel_new", forge_ui::tr!("Cancel"))?;
        pb.b.hide(form, true);

        // ---- unsaved changes ----
        let unsaved = group(pb, root, "unsaved", forge_ui::tr!("Unsaved changes"))?;
        let unsaved_text = pb.b.signal(String::new());
        text(pb, unsaved, "unsaved_text", unsaved_text, LabelKind::Body)?;
        let ubar = bar(pb, unsaved, "unsaved_bar", forge_ui::tr!("Unsaved changes actions"))?;
        let save_first = button(pb, ubar, "save_first", forge_ui::tr!("Save first"))?;
        let discard = button(pb, ubar, "discard", forge_ui::tr!("Discard changes and continue"))?;
        let keep = button(pb, ubar, "keep", forge_ui::tr!("Cancel"))?;
        pb.b.hide(unsaved, true);

        let status = pb.b.signal(String::new());
        text(pb, root, "status", status, LabelKind::Muted)?;

        // ---- behaviour ----
        // Send `op`, or ask first when the open project has unsaved changes.
        let send = {
            let state = state.clone();
            move |act: &mut forge_editor::panel_rt::PanelAct, op: ProjectOp| {
                let dirty = act
                    .mirror
                    .project()
                    .and_then(|s| s.open.as_ref())
                    .is_some_and(|o| o.dirty);
                if dirty {
                    let name = act
                        .mirror
                        .project()
                        .and_then(|s| s.open.as_ref())
                        .map(|o| o.name.clone())
                        .unwrap_or_default();
                    set(
                        act.ui,
                        unsaved_text,
                        forge_ui::trf!("\u{201c}{name}\u{201d} has unsaved changes. Save them first, or discard them?", name),
                    );
                    state.borrow_mut().pending = Some(op);
                    show(act.ui, unsaved, true);
                } else {
                    act.cmd.emit(op.command());
                    set(act.ui, status, format!("{}\u{2026}", forge_editor::project::op_title(op.target())));
                }
            }
        };
        pb.on(new, move |act, _: &Pressed| show(act.ui, form, true));
        pb.on(cancel_new, move |act, _: &Pressed| show(act.ui, form, false));
        {
            let state = state.clone();
            pb.on(templates, move |_act, e: &SelectionChanged| {
                let mut st = state.borrow_mut();
                st.template = e
                    .keys
                    .first()
                    .and_then(|k| st.templates.get(*k as usize).cloned());
            });
        }
        {
            let (state, send) = (state.clone(), send.clone());
            pb.on(create, move |act, _: &Pressed| {
                let Some(template) = state.borrow().template.clone() else {
                    act.session.problem(forge_editor::notify::Problem::coded(forge_editor::notify::Severity::Warning, "EDITOR-0014", forge_ui::tr!("Pick a template"), forge_ui::tr!("Choose a template first.")));
                    return;
                };
                let n = name.get(act.ui.rt()).trim().to_string();
                let n = if n.is_empty() { forge_ui::l10n::tr(DEFAULT_NAME).to_string() } else { n };
                let typed = location.get(act.ui.rt()).trim().to_string();
                let loc = if typed.is_empty() {
                    let dir = act.services.launcher.borrow().projects_dir.clone();
                    unique_location(&dir, &n)
                } else if ["file:", "git:", "memory:"].iter().any(|s| typed.starts_with(s)) {
                    typed
                } else {
                    format!("file:{typed}")
                };
                send(
                    act,
                    ProjectOp::Create {
                        location: loc,
                        name: n,
                        template,
                        discard_unsaved: false,
                    },
                );
            });
        }
        let open_at = {
            let send = send.clone();
            move |act: &mut forge_editor::panel_rt::PanelAct, loc: String| {
                let loc = if ["file:", "git:", "memory:"].iter().any(|s| loc.starts_with(s)) {
                    loc
                } else {
                    format!("file:{loc}")
                };
                send(
                    act,
                    ProjectOp::Open {
                        location: loc,
                        discard_unsaved: false,
                    },
                );
            }
        };
        {
            let open_at = open_at.clone();
            pb.on(open, move |act, _: &Pressed| {
                let loc = open_loc.get(act.ui.rt()).trim().to_string();
                if loc.is_empty() {
                    act.session.problem(forge_editor::notify::Problem::coded(forge_editor::notify::Severity::Warning, "EDITOR-0014", forge_ui::tr!("Nothing to open"), forge_ui::tr!("Type the project's location (its folder) first.")));
                    return;
                }
                open_at(act, loc);
            });
        }
        let _ = open_field;
        {
            let send = send.clone();
            pb.on(clone, move |act, _: &Pressed| {
                let remote = clone_url.get(act.ui.rt()).trim().to_string();
                if remote.is_empty() {
                    act.session.problem(forge_editor::notify::Problem::coded(forge_editor::notify::Severity::Warning, "EDITOR-0014", forge_ui::tr!("Nothing to clone"), forge_ui::tr!("Type the remote's location first, e.g. file:\\\\nas\\share\\my-game.")));
                    return;
                }
                // A URL goes as typed (the core checks it); a path to a Git repository is
                // `git-file:`, any other path a Forge folder (WP-16).
                let remote = forge_project::host::remote_url(&remote);
                // The new folder is named after the remote's last path segment.
                let tail = forge_project::host::clone_name(&remote);
                let dir = act.services.launcher.borrow().projects_dir.clone();
                send(
                    act,
                    ProjectOp::Clone {
                        remote,
                        location: unique_location(&dir, &tail),
                        discard_unsaved: false,
                    },
                );
            });
        }
        let selected_recent = move |ui: &mut forge_ui::Ui, state: &State| -> Option<String> {
            VirtualTree::edit(ui, recent, |t| t.selected().first().copied())
                .flatten()
                .and_then(|k| state.recent.get(k as usize))
                .map(|e| e.location.clone())
        };
        {
            let (state, open_at) = (state.clone(), open_at.clone());
            pb.on(recent, move |act, e: &RowActivated| {
                let loc = state
                    .borrow()
                    .recent
                    .get(e.key as usize)
                    .map(|r| r.location.clone());
                if let Some(loc) = loc {
                    open_at(act, loc);
                }
            });
        }
        {
            let (state, open_at) = (state.clone(), open_at.clone());
            pb.on(open_recent, move |act, _: &Pressed| {
                let loc = selected_recent(act.ui, &state.borrow());
                if let Some(loc) = loc {
                    open_at(act, loc);
                }
            });
        }
        {
            let state = state.clone();
            pb.on(forget, move |act, _: &Pressed| {
                let loc = selected_recent(act.ui, &state.borrow());
                if let Some(loc) = loc
                    && let Err(e) = act.services.launcher.borrow_mut().forget(&loc)
                {
                    act.session.problem(forge_editor::notify::Problem::coded(forge_editor::notify::Severity::Warning, e.code(), forge_ui::tr!("The recent projects list was not saved"), &e.to_string()));
                }
            });
        }
        {
            let state = state.clone();
            pb.on(save_first, move |act, _: &Pressed| {
                // The core runs them in order: the save lands before the create or open.
                if let Some(op) = state.borrow_mut().pending.take() {
                    act.cmd.emit(save_command(""));
                    act.cmd.emit(op.command());
                }
                show(act.ui, unsaved, false);
            });
        }
        {
            let state = state.clone();
            pb.on(discard, move |act, _: &Pressed| {
                if let Some(op) = state.borrow_mut().pending.take() {
                    let op = match op {
                        ProjectOp::Create {
                            location,
                            name,
                            template,
                            ..
                        } => ProjectOp::Create {
                            location,
                            name,
                            template,
                            discard_unsaved: true,
                        },
                        ProjectOp::Open { location, .. } => ProjectOp::Open {
                            location,
                            discard_unsaved: true,
                        },
                        ProjectOp::Clone {
                            remote, location, ..
                        } => ProjectOp::Clone {
                            remote,
                            location,
                            discard_unsaved: true,
                        },
                        other => other,
                    };
                    act.cmd.emit(op.command());
                }
                show(act.ui, unsaved, false);
            });
        }
        {
            let state = state.clone();
            pb.on(keep, move |act, _: &Pressed| {
                state.borrow_mut().pending = None;
                show(act.ui, unsaved, false);
            });
        }

        // Follow the recent list, where a new project would go, and create/open outcomes.
        let mut seen_recent = u64::MAX;
        let mut seen_name = String::new();
        let mut seen_outcome = pb
            .mirror()
            .project()
            .and_then(|s| s.outcomes.last().map(|o| o.id))
            .unwrap_or(0);
        let mut seen_catalog = catalog_rev;
        pb.sync(root, move |s| {
            // A preset plugin installed while the editor runs joins the list (its install
            // posts a notice, so this step runs; the catalog's revision says whether to look).
            let rev = s
                .services
                .hosting
                .borrow()
                .presets()
                .read()
                .map_or(seen_catalog, |c| c.revision());
            if rev != seen_catalog {
                seen_catalog = rev;
                let (listed, rows, _) = template_rows(s.services);
                VirtualTree::edit(s.ui, templates, |t| {
                    let sel = t.selected();
                    t.clear();
                    for (i, label) in rows.into_iter().enumerate() {
                        t.push(None, i as u64, RowItem::new(label));
                    }
                    t.select(&sel);
                });
                state.borrow_mut().templates = listed;
            }
            let l = s.services.launcher.borrow();
            if l.revision() != seen_recent {
                seen_recent = l.revision();
                let entries = l.recent.entries.clone();
                show(s.ui, empty, entries.is_empty());
                show(s.ui, recent, !entries.is_empty());
                VirtualTree::edit(s.ui, recent, |t| {
                    t.clear();
                    for (i, e) in entries.iter().enumerate() {
                        t.push(None, i as u64, RowItem::new(recent_label(e)));
                    }
                });
                state.borrow_mut().recent = entries;
            }
            let n = name.get(s.ui.rt());
            if n != seen_name {
                seen_name = n.clone();
                let n = if n.trim().is_empty() {
                    forge_ui::l10n::tr(DEFAULT_NAME).to_string()
                } else {
                    n.trim().to_string()
                };
                set(
                    s.ui,
                    where_,
                    forge_ui::trf!("Will be created at {unique_location} (unless you type a folder above).", unique_location = unique_location(&l.projects_dir, &n)),
                );
            }
            drop(l);
            if let Some(st) = s.mirror.project() {
                // A clone runs off the core's lock: say so until its outcome arrives.
                if let Some(t) = &st.transfer
                    && t.op == CLONE_CMD
                {
                    set(s.ui, status, format!("{}\u{2026}", t.what));
                }
                let from = seen_outcome;
                for o in st.outcomes.iter().filter(|o| o.id > from) {
                    seen_outcome = o.id;
                    if o.op != CREATE_CMD && o.op != OPEN_CMD && o.op != CLONE_CMD {
                        continue;
                    }
                    match &o.result {
                        Ok(msg) => {
                            set(s.ui, status, msg.clone());
                            if o.op == CREATE_CMD {
                                show(s.ui, form, false);
                            }
                        }
                        Err((code, msg)) => set(s.ui, status, format!("{code}: {msg}")),
                    }
                }
            }
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}
