//! **Revision history and store** (`forge.history`, Ch.33 §33.4, DoD M2-50).
//!
//! The history is **semantic**: each revision lists what every issuer did, read from its
//! command envelopes ("automation:sess-4: placed 37 “Tree”"), not which files changed. Save
//! (commit), push and pull are **commands** (`forge.project.save` / `push` / `pull`) the
//! core performs (E-36); linking a remote is `forge.project.link_remote`, an undoable
//! project edit whose URL the core checks. The panel never touches a store.
//!
//! After the project's first save, when no remote is linked, the panel offers to link one
//! (O-11: the offer comes at the first save, not at creation). "Not now" dismisses it.
//!
//! **Remotes (WP-16).** A remote is a Git host (`https://github.com/you/game.git`, GitLab,
//! Gitea, Forgejo), a bare Git repository on a share, or a Forge folder; a typed path becomes
//! the right URL (`forge_project::host::remote_url`). **GitHub** is one group: *Sign in*
//! starts the device flow (`forge.project.sign_in`) and shows the code to enter and where;
//! *Continue* asks GitHub whether it was entered; *Create on GitHub* makes a private
//! repository named after the project and links it (`forge.project.create_remote`). The
//! token stays in the core: nothing here or on the bus ever holds it.

use std::cell::RefCell;
use std::rc::Rc;

use forge_editor::panels::PanelCx;
use forge_editor::project::{
    CREATE_REMOTE_CMD, PULL_CMD, PUSH_CMD, ProjectOp, REMOTE_SETTING, RevisionInfo, SAVE_CMD,
    SIGN_IN_CMD, link_remote_command, op_title, save_command,
};
use forge_project::host::remote_url;
use forge_ui::widgets::{
    Button, EmptyState, LabelKind, Pressed, RowItem, Submitted, TextField, VirtualTree,
};
use forge_ui::{NodeStyle, Role};

use crate::ui::{bar, button, group, set, show, text};

fn clock(ms: u64) -> String {
    let s = (ms / 1000) % 86_400;
    forge_ui::trf!(
        "{time} UTC",
        time = format!("{:02}:{:02}", s / 3600, (s / 60) % 60)
    )
}

/// "1 revision" or "3 revisions".
fn revision_count(n: usize) -> String {
    if n == 1 {
        forge_ui::tr!("1 revision").into()
    } else {
        forge_ui::trf!("{n} revisions", n)
    }
}

/// A revision's row: short id, message, issuers and time.
pub fn revision_label(r: &RevisionInfo) -> String {
    format!(
        "{} \u{2014} {} \u{2014} {} \u{b7} {}",
        r.short,
        r.message,
        if r.issuers.is_empty() {
            forge_ui::tr!("no commands").to_string()
        } else {
            r.issuers.join(", ")
        },
        clock(r.at_ms)
    )
}

/// A GitHub repository name for a project named `name` (`My Orbits!` → `my-orbits`).
pub fn repo_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.') {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches(['-', '.']).to_string();
    out.chars().take(100).collect()
}

#[derive(Default)]
struct State {
    /// The head the "link a remote" offer was dismissed at.
    dismissed_at: Option<String>,
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let fault_always_want_turn = pb.services().faults.history_always_want_turn();
        let space = pb.b.theme_ref().space;
        let root = pb.b.add(
            pb.parent,
            "content",
            NodeStyle::column(space[1])
                .padding(space[2])
                .grow(1.0)
                .scrollable(),
            forge_ui::widgets::Container::new(Role::Group).labelled(forge_ui::tr!("Revision history")),
        )?;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Heading)?;

        let offer = group(pb, root, "offer", forge_ui::tr!("Link a remote"))?;
        text(
            pb,
            offer,
            "offer_text",
            forge_ui::tr!("Saved for the first time. Link a remote to back the project up and share it \u{2014} GitHub (sign in below), any Git host, a bare repository on a share, or a folder."),
            LabelKind::Body,
        )?;
        let obar = bar(pb, offer, "offer_bar", forge_ui::tr!("Link a remote now or later"))?;
        let offer_link = pb.b.add(
            obar,
            "offer_link",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Link a remote\u{2026}")).primary(),
        )?;
        let offer_dismiss = button(pb, obar, "offer_dismiss", forge_ui::tr!("Not now"))?;
        pb.b.hide(offer, true);

        let sbar = bar(pb, root, "save_bar", forge_ui::tr!("Save"))?;
        let message = pb.b.signal(String::new());
        pb.b.add(
            sbar,
            "message",
            NodeStyle::leaf().width(300.0),
            TextField::new(message, forge_ui::tr!("Revision message"))
                .placeholder(forge_ui::tr!("What changed (optional: the summary is used)")),
        )?;
        let save = pb.b.add(sbar, "save", NodeStyle::leaf(), Button::new(forge_ui::tr!("Save")).primary())?;

        let rg = group(pb, root, "remote_group", forge_ui::tr!("Remote"))?;
        let remote_state = pb.b.signal(String::new());
        text(pb, rg, "remote_state", remote_state, LabelKind::Body)?;
        let rbar = bar(pb, rg, "remote_bar", forge_ui::tr!("Remote actions"))?;
        let url = pb.b.signal(String::new());
        let url_field = pb.b.add(
            rbar,
            "remote_url",
            NodeStyle::leaf().width(260.0),
            TextField::new(url, forge_ui::tr!("Remote URL"))
                .placeholder(forge_ui::tr!("https://github.com/you/my-game.git, or a folder")),
        )?;
        let link = button(pb, rbar, "link", forge_ui::tr!("Link"))?;
        let push = button(pb, rbar, "push", forge_ui::tr!("Push"))?;
        let pull = button(pb, rbar, "pull", forge_ui::tr!("Pull"))?;
        text(
            pb,
            rg,
            "remote_hint",
            forge_ui::tr!("A remote is a Git host over HTTPS (GitHub, GitLab, Gitea, Forgejo), a bare Git repository on a share (its path), or a Forge folder. Text files are diffable on the host; binaries travel as engine blobs."),
            LabelKind::Small,
        )?;
        // GitHub: sign in with the device flow, then create the repository in one click.
        let gh = group(pb, root, "github", forge_ui::tr!("GitHub"))?;
        let gh_state = pb.b.signal(String::from(forge_ui::tr!("Not signed in.")));
        text(pb, gh, "github_state", gh_state, LabelKind::Body)?;
        let gbar = bar(pb, gh, "github_bar", forge_ui::tr!("GitHub actions"))?;
        let sign_in = button(pb, gbar, "sign_in", forge_ui::tr!("Sign in to GitHub"))?;
        let sign_in_continue = button(pb, gbar, "sign_in_continue", forge_ui::tr!("Continue"))?;
        let create_remote = button(pb, gbar, "create_remote", forge_ui::tr!("Create on GitHub"))?;

        let empty = pb.b.add(
            root,
            "empty",
            NodeStyle::leaf(),
            EmptyState::new(
                forge_ui::tr!("No revisions yet. Save to make the first: each revision records what every person and automation session did."),
            ),
        )?;
        let list = pb.b.add(
            root,
            "revisions",
            NodeStyle::leaf().grow(1.0).height(200.0),
            VirtualTree::list(forge_ui::tr!("Revisions")).single_select(),
        )?;
        pb.b.hide(list, true);
        let status = pb.b.signal(String::new());
        text(pb, root, "status", status, LabelKind::Muted)?;

        let state = Rc::new(RefCell::new(State::default()));
        pb.on(save, move |act, _: &Pressed| {
            let m = message.get(act.ui.rt());
            act.cmd.emit(save_command(m.trim()));
            set(act.ui, message, String::new());
        });
        // Enter in the URL field links too.
        pb.on(url_field, move |act, e: &Submitted| {
            let u = e.text.trim().to_string();
            if !u.is_empty() {
                act.cmd.emit(link_remote_command(&remote_url(&u)));
            }
        });
        pb.on(sign_in, move |act, _: &Pressed| {
            act.cmd.emit(ProjectOp::SignIn { resume: false }.command());
        });
        pb.on(sign_in_continue, move |act, _: &Pressed| {
            act.cmd.emit(ProjectOp::SignIn { resume: true }.command());
        });
        pb.on(create_remote, move |act, _: &Pressed| {
            let name = act
                .mirror
                .project()
                .and_then(|s| s.open.as_ref())
                .map(|o| repo_name(&o.name))
                .unwrap_or_default();
            if name.is_empty() {
                act.session.problem(forge_editor::notify::Problem::coded(forge_editor::notify::Severity::Warning, "EDITOR-0014", forge_ui::tr!("No project"), forge_ui::tr!("Create or open a project first: the repository is named after it.")));
                return;
            }
            act.cmd.emit(ProjectOp::CreateRemote { name }.command());
        });
        pb.on(link, move |act, _: &Pressed| {
            let u = url.get(act.ui.rt()).trim().to_string();
            if u.is_empty() {
                act.session.problem(forge_editor::notify::Problem::coded(forge_editor::notify::Severity::Warning, "EDITOR-0014", forge_ui::tr!("No remote URL"), forge_ui::tr!("Type the remote's location first, e.g. file:\\\\nas\\share\\my-game.")));
                return;
            }
            act.cmd.emit(link_remote_command(&remote_url(&u)));
        });
        pb.on(push, move |act, _: &Pressed| {
            act.cmd.emit(ProjectOp::Push.command());
        });
        pb.on(pull, move |act, _: &Pressed| {
            act.cmd.emit(ProjectOp::Pull.command());
        });
        pb.on(offer_link, move |act, _: &Pressed| {
            act.ui.set_focus(Some(url_field), true);
        });
        {
            let state = state.clone();
            pb.on(offer_dismiss, move |act, _: &Pressed| {
                state.borrow_mut().dismissed_at = act
                    .mirror
                    .project()
                    .and_then(|s| s.open.as_ref())
                    .and_then(|o| o.head.clone());
                show(act.ui, offer, false);
            });
        }

        let mut seen = (u64::MAX, u64::MAX);
        let mut seen_head: Option<Option<String>> = None;
        // The epoch a full rebuild last saw, and how many revisions it built. A save only
        // ever inserts one new revision at the front (`ProjectHost::commit`) without
        // bumping the epoch; anything that replaces the history instead (open, create, a
        // pull that moved) bumps it (`ProjectStatus::epoch`'s own contract) and is caught
        // by the `!=` below, which forces the safe full-rebuild fallback.
        let mut seen_epoch = u64::MAX;
        let mut seen_len: usize = 0;
        let mut next_slot: u64 = 0;
        let mut seen_outcome = pb
            .mirror()
            .project()
            .and_then(|s| s.outcomes.last().map(|o| o.id))
            .unwrap_or(0);
        pb.sync(root, move |s| {
            if fault_always_want_turn {
                s.want_turn();
            }
            let rev = (s.mirror.project_revision(), s.mirror.settings_revision());
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            let remote = match s.mirror.setting(REMOTE_SETTING) {
                Some(forge_cmd::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
                _ => None,
            };
            let st = s.mirror.project();
            let open = st.and_then(|p| p.open.as_ref());
            match open {
                None => {
                    set(
                        s.ui,
                        head,
                        forge_ui::tr!("No project is open. Create or open one in the Launcher.").into(),
                    );
                }
                Some(o) => set(
                    s.ui,
                    head,
                    forge_ui::trf!(
                        "{name} \u{b7} {revisions}{unsaved}",
                        name = o.name,
                        revisions = revision_count(o.revisions.len()),
                        unsaved = if o.dirty {
                            forge_ui::tr!(" \u{b7} unsaved changes")
                        } else {
                            ""
                        }
                    ),
                ),
            }
            let remote_head = open.and_then(|o| o.remote_head.clone());
            set(
                s.ui,
                remote_state,
                match (&remote, remote_head) {
                    (None, _) => forge_ui::tr!("Not linked: the project is on this computer only.").into(),
                    (Some(u), None) => forge_ui::trf!("Linked: {unit}", unit = u),
                    (Some(u), Some(h)) => {
                        forge_ui::trf!("Linked: {unit} \u{b7} in step at {chars}", unit = u, chars = h.chars().take(10).collect::<String>())
                    }
                },
            );
            let signed = st.map(|p| (p.sign_in.clone(), p.accounts.clone()));
            set(
                s.ui,
                gh_state,
                match signed {
                    Some((Some(si), _)) if si.account.is_none() => forge_ui::trf!("Enter {error} at {detail}, then press Continue.", error = si.user_code.unwrap_or_default(), detail = si.verification_uri.unwrap_or_default()),
                    Some((_, accounts)) if !accounts.is_empty() => {
                        forge_ui::trf!("Signed in: {items}.", items = accounts.join(", "))
                    }
                    _ => forge_ui::tr!("Not signed in.").into(),
                },
            );
            let offered = open.is_some_and(|o| {
                o.offer_remote
                    && remote.is_none()
                    && state.borrow().dismissed_at != o.head.clone()
            });
            show(s.ui, offer, offered);
            let h = open.map(|o| o.head.clone());
            let flat_h = h.flatten();
            if seen_head.as_ref() != Some(&flat_h) {
                let epoch = st.map(|p| p.epoch).unwrap_or(0);
                let revs_now = open.map(|o| o.revisions.len()).unwrap_or(0);
                show(s.ui, empty, revs_now == 0);
                show(s.ui, list, revs_now != 0);
                // A plain save inserts exactly one revision at the front and leaves every
                // other row (and its summary lines) untouched: add just that row instead
                // of cloning and rebuilding the whole history (`rows_written` in the test
                // asserts this costs one row, not the list).
                let prepend_only = epoch == seen_epoch && revs_now == seen_len + 1;
                if prepend_only {
                    if let Some(r) = open.and_then(|o| o.revisions.first()) {
                        let k = next_slot << 8;
                        next_slot += 1;
                        VirtualTree::edit(s.ui, list, |t| {
                            t.insert(None, 0, k, RowItem::new(revision_label(r)));
                            for (j, line) in r.summary.iter().enumerate().take(255) {
                                t.push(Some(k), k | (j as u64 + 1), RowItem::new(line.clone()).muted(true));
                            }
                            t.set_expanded(k, true);
                        });
                    }
                } else {
                    let revs: Vec<RevisionInfo> =
                        open.map(|o| o.revisions.clone()).unwrap_or_default();
                    VirtualTree::edit(s.ui, list, |t| {
                        t.clear();
                        for (i, r) in revs.iter().enumerate() {
                            let k = (i as u64) << 8;
                            t.push(None, k, RowItem::new(revision_label(r)));
                            for (j, line) in r.summary.iter().enumerate().take(255) {
                                t.push(Some(k), k | (j as u64 + 1), RowItem::new(line.clone()).muted(true));
                            }
                            t.set_expanded(k, true);
                        }
                    });
                    next_slot = revs.len() as u64;
                }
                seen_head = Some(flat_h);
                seen_epoch = epoch;
                seen_len = revs_now;
            }
            if let Some(st) = st {
                // A transfer runs off the core's lock: say what it is doing until its
                // outcome arrives (the editor stays usable meanwhile).
                if let Some(t) = &st.transfer
                    && [PUSH_CMD, PULL_CMD, SIGN_IN_CMD, CREATE_REMOTE_CMD].contains(&t.op.as_str())
                {
                    set(s.ui, status, format!("{}\u{2026}", t.what));
                }
                let from = seen_outcome;
                for o in st.outcomes.iter().filter(|o| o.id > from) {
                    seen_outcome = o.id;
                    if ![SAVE_CMD, PUSH_CMD, PULL_CMD, SIGN_IN_CMD, CREATE_REMOTE_CMD]
                        .contains(&o.op.as_str())
                    {
                        continue;
                    }
                    let line = match &o.result {
                        Ok(m) => format!("{}: {m}", op_title(&o.op)),
                        Err((c, m)) => forge_ui::trf!(
                            "{op} failed ({c}): {m}",
                            op = op_title(&o.op),
                            c,
                            m
                        ),
                    };
                    set(s.ui, status, line);
                }
            }
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}
