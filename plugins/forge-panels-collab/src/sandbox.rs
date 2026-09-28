//! **Sandbox and Live/Pull** (`forge.sandbox`, Ch.37 §37.2–§37.3; DoD M2-58).
//!
//! * Your sandbox (I19: `project_view = baseline ⊕ sandbox_deltas`): the baseline revision
//!   it is on and what is not published yet, summarised ("placed 3 “Tree”").
//! * **Live / Pull, one click, reversible at any time** (I20: two policies over one stream,
//!   the switch changes when you see a publish, never what you end up with). Session state:
//!   your own subscription, not project state.
//! * In Pull: the revisions waiting, oldest first — pull everything, or up to the one you
//!   select ("pull any pushed work of your choosing").
//! * Visibility (default `Team`, O-17): Private (you and the team's Owners), Team, Shared.
//! * Teammates' sandboxes as you may see them (a private one says so and shows nothing).
//! * The plain statement E-37 requires.

use std::cell::RefCell;
use std::rc::Rc;

use forge_editor::collab as c;
use forge_editor::panels::PanelCx;
use forge_project::collab::{BaselineRev, Policy, Visibility, short};
use forge_store::RevId;
use forge_ui::widgets::{LabelKind, Pressed};
use forge_ui::{Signal, Ui, WidgetId};

use crate::ui::{
    bar, button, feed_once, fill, frame, group, last_outcome, list, notice, primary, selected, set,
    show, text,
};

struct Parts {
    head: Signal<String>,
    changes: WidgetId,
    policy: Signal<String>,
    policy_button: Signal<String>,
    visibility: Signal<String>,
    incoming_head: Signal<String>,
    incoming: WidgetId,
    pull_bar: WidgetId,
    sandboxes: WidgetId,
    controls: Vec<WidgetId>,
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let f = frame(pb, forge_ui::tr!("Sandbox"))?;
        let root = f.root;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Heading)?;
        let changes = list(
            pb,
            root,
            "changes",
            forge_ui::tr!("Unpublished changes"),
            70.0,
        )?;

        let pbar = bar(pb, root, "policy_bar", forge_ui::tr!("Live or Pull"))?;
        let policy = pb.b.signal(String::new());
        text(pb, pbar, "policy", policy, LabelKind::Body)?;
        let policy_button = pb.b.signal(forge_ui::tr!("Switch to Live").to_string());
        // One click, reversible at any time; the label says what the click does.
        let toggle = pb.b.add(
            pbar,
            "policy_toggle",
            forge_ui::NodeStyle::leaf(),
            forge_ui::widgets::Button::new(policy_button).primary(),
        )?;

        let vbar = bar(
            pb,
            root,
            "visibility_bar",
            forge_ui::tr!("Who may read your sandbox"),
        )?;
        let visibility = pb.b.signal(String::new());
        text(pb, vbar, "visibility", visibility, LabelKind::Body)?;
        let vis: Vec<(WidgetId, Visibility)> = vec![
            (
                button(pb, vbar, "vis_private", forge_ui::tr!("Private"))?,
                Visibility::Private,
            ),
            (
                button(pb, vbar, "vis_team", forge_ui::tr!("Team"))?,
                Visibility::Team,
            ),
            (
                button(pb, vbar, "vis_shared", forge_ui::tr!("Shared (read-only)"))?,
                Visibility::Shared,
            ),
        ];

        let ig = group(pb, root, "incoming_group", forge_ui::tr!("From the team"))?;
        let incoming_head = pb.b.signal(String::new());
        text(pb, ig, "incoming_head", incoming_head, LabelKind::Body)?;
        let incoming = list(pb, ig, "incoming", forge_ui::tr!("Revisions to pull"), 80.0)?;
        let pull_bar = bar(pb, ig, "pull_bar", forge_ui::tr!("Pull"))?;
        let pull_all = primary(pb, pull_bar, "pull_all", forge_ui::tr!("Pull everything"))?;
        let pull_to = button(
            pb,
            pull_bar,
            "pull_to",
            forge_ui::tr!("Pull up to the selected"),
        )?;

        let tg = group(pb, root, "teammates", forge_ui::tr!("Teammates' sandboxes"))?;
        let sandboxes = list(
            pb,
            tg,
            "sandboxes",
            forge_ui::tr!("Teammates' sandboxes"),
            80.0,
        )?;
        text(
            pb,
            root,
            "e37",
            forge_ui::l10n::tr(c::E37_STATEMENT),
            LabelKind::Small,
        )?;
        text(pb, root, "status", f.status, LabelKind::Muted)?;

        let revs: Rc<RefCell<Vec<BaselineRev>>> = Rc::new(RefCell::new(Vec::new()));
        let parts = Rc::new(Parts {
            head,
            changes,
            policy,
            policy_button,
            visibility,
            incoming_head,
            incoming,
            pull_bar,
            sandboxes,
            controls: vec![pbar, vbar],
        });

        pb.on(toggle, move |act, _: &Pressed| {
            let now = act.services.collab.policy();
            act.services.collab.set_policy(match now {
                Policy::Live => Policy::Pull,
                Policy::Pull => Policy::Live,
            });
            act.want_turn();
        });
        for (id, v) in vis {
            pb.on(id, move |act, _: &Pressed| {
                act.services.collab.set_visibility(v);
                act.want_turn();
            });
        }
        pb.on(pull_all, move |act, _: &Pressed| {
            act.cmd.emit(c::pull_command(None));
            act.want_turn();
        });
        {
            let revs = revs.clone();
            pb.on(pull_to, move |act, _: &Pressed| {
                let rev: Option<RevId> = selected(act.ui, incoming)
                    .and_then(|k| revs.borrow().get(k as usize).map(|r| r.id));
                if let Some(r) = rev {
                    act.cmd.emit(c::pull_command(Some(r)));
                }
                act.want_turn();
            });
        }

        let mut feeds = false;
        let mut seen = u64::MAX;
        pb.sync(root, move |s| {
            feed_once(s, f.relay, &mut feeds, crate::LIVE_HZ);
            let rev = s.services.collab.revision() ^ s.mirror.revision().rotate_left(9);
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            let ok = notice(s.ui, s.services, &f);
            refresh(s.ui, s.services, &parts, &revs, ok);
            set(
                s.ui,
                f.status,
                last_outcome(
                    s.services,
                    &[c::PULL, forge_ui::tr!("Live pull"), c::RESOLVE],
                ),
            );
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}

fn refresh(
    ui: &mut Ui,
    services: &forge_editor::services::EditorServices,
    p: &Parts,
    revs: &Rc<RefCell<Vec<BaselineRev>>>,
    ok: bool,
) {
    let collab = &services.collab;
    let Some(st) = collab.status() else {
        set(
            ui,
            p.head,
            forge_ui::tr!("No team server is attached.").into(),
        );
        for id in &p.controls {
            show(ui, *id, false);
        }
        show(ui, p.pull_bar, false);
        return;
    };
    if !st.joined {
        set(
            ui,
            p.head,
            forge_ui::tr!(
                "This project does not follow a team baseline: create or join a team in Team."
            )
            .into(),
        );
    } else {
        set(
            ui,
            p.head,
            forge_ui::trf!(
                "Your sandbox: {unpublished} unpublished change(s) on baseline revision {short}",
                unpublished = st.unpublished,
                short = short(st.base)
            ),
        );
    }
    fill(
        ui,
        p.changes,
        st.summary
            .iter()
            .enumerate()
            .map(|(i, l)| (i as u64, l.clone(), false))
            .collect(),
    );
    let policy = collab.policy();
    set(
        ui,
        p.policy,
        match policy {
            Policy::Live => {
                forge_ui::tr!("Live: your view follows each publish as it lands.").into()
            }
            Policy::Pull => {
                forge_ui::tr!("Pull: your view moves when you pull, up to the revision you choose.")
                    .into()
            }
        },
    );
    set(
        ui,
        p.policy_button,
        match policy {
            Policy::Live => forge_ui::tr!("Switch to Pull").into(),
            Policy::Pull => forge_ui::tr!("Switch to Live").into(),
        },
    );
    let me = collab.me();
    let vis = collab
        .server
        .as_ref()
        .map_or_else(Visibility::default, |s| s.visibility(&me));
    set(
        ui,
        p.visibility,
        forge_ui::trf!(
            "Your sandbox is {label}.",
            label = forge_ui::l10n::tr(vis.label())
        ),
    );
    for id in &p.controls {
        show(ui, *id, ok && st.joined);
    }
    // Incoming revisions (after my base).
    let incoming = collab
        .server
        .as_ref()
        .map(|s| s.revisions(st.base, 200))
        .unwrap_or_default();
    let waiting = if st.joined { incoming.len() } else { 0 };
    set(
        ui,
        p.incoming_head,
        match (waiting, policy, &st.pending) {
            (_, _, Some(pp)) => forge_ui::trf!(
                "A pull stopped at {max} conflict(s): resolve them in Conflicts.",
                max = pp
                    .conflicts
                    .iter()
                    .filter(|c| c.choice.is_none())
                    .count()
                    .max(1)
            ),
            (0, _, _) => forge_ui::tr!("Up to date with the team baseline.").into(),
            (n, Policy::Live, _) => forge_ui::trf!("{n} revision(s) arriving (Live).", n),
            (n, Policy::Pull, _) => forge_ui::trf!("{n} revision(s) to pull.", n),
        },
    );
    fill(
        ui,
        p.incoming,
        incoming
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let what = r
                    .summary
                    .first()
                    .cloned()
                    .unwrap_or_else(|| r.message.clone());
                (
                    i as u64,
                    forge_ui::trf!(
                        "{rev} \u{2014} {who} \u{2014} {what} ({n} command(s))",
                        rev = short(Some(r.id)),
                        who = r.issuers.join(", "),
                        what,
                        n = r.commands
                    ),
                    !ok,
                )
            })
            .collect(),
    );
    show(ui, p.pull_bar, ok && st.joined && waiting > 0);
    *revs.borrow_mut() = incoming;
    // Teammates' sandboxes, as I may see them.
    let rows = collab
        .server
        .as_ref()
        .map(|s| s.sandboxes(&me))
        .unwrap_or_default();
    fill(
        ui,
        p.sandboxes,
        rows.iter()
            .filter(|r| r.owner != me)
            .enumerate()
            .map(|(i, r)| {
                let line = if r.withheld {
                    forge_ui::trf!("{owner} \u{2014} private sandbox", owner = r.owner)
                } else {
                    forge_ui::trf!(
                        "{owner} \u{2014} {visibility} \u{2014} {policy} \u{2014} {n} unpublished{summary}",
                        owner = r.owner,
                        visibility = forge_ui::l10n::tr(r.visibility.label()),
                        policy = forge_ui::l10n::tr(r.policy.label()),
                        n = r.unpublished,
                        summary = r.summary
                            .first()
                            .map(|s| format!(": {s}"))
                            .unwrap_or_default()
                    )
                };
                (i as u64, line, r.withheld || !ok)
            })
            .collect(),
    );
}
