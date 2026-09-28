//! **Publish and review queue** (`forge.publish_queue`, Ch.37 §37.3, O-16; DoD M2-60,
//! M6-13).
//!
//! * **Publishing is a command** (`forge.collab.publish`): the core commits the sandbox to the
//!   team baseline with `ProjectStore::commit` (E-36) — the panel never calls the store. It
//!   is a fast-forward: when the baseline moved, pull first (Live does it for you).
//! * **Optional approval** (O-16): Developers publish directly unless the project requires
//!   approval; per-path rules on top (`scene/Sandbox/**` direct, `engine-config/**`
//!   reviewed). A gated publish waits here until an Owner, a Maintainer or a Reviewer — not its
//!   author — approves (it is committed then) or rejects it with a reason.
//! * Publish scopes: a member scoped to `scene/Levels/**` is refused, naming the path, when
//!   their change reaches outside it.

use std::cell::RefCell;
use std::rc::Rc;

use forge_editor::collab as c;
use forge_editor::panels::PanelCx;
use forge_project::collab::{Request, RequestState, Role};
use forge_ui::widgets::{LabelKind, Pressed};
use forge_ui::{Signal, Ui, WidgetId};

/// A request's state as the queue shows it (`forge-project` words it for logs; the panel
/// looks its own words up).
fn state_label(s: &RequestState) -> String {
    match s {
        RequestState::Pending => forge_ui::tr!("waiting for review").into(),
        RequestState::Approved { by, .. } => forge_ui::trf!("approved by {by}", by),
        RequestState::Rejected { by, reason } => {
            forge_ui::trf!("rejected by {by}: {reason}", by, reason)
        }
        RequestState::Stale(why) => forge_ui::trf!("stale: {why}", why),
    }
}

use crate::ui::{
    bar, button, feed_once, field, fill, frame, group, last_outcome, list, notice, primary,
    selected, set, show, text,
};

const TARGETS: &[&str] = &[c::PUBLISH, c::APPROVE, c::REJECT];

/// Parse review rules typed as `glob=review` / `glob=direct`, comma- or line-separated.
#[must_use]
pub fn parse_rules(s: &str) -> Vec<(String, bool)> {
    s.split([',', '\n'])
        .filter_map(|r| {
            let (g, v) = r.split_once('=')?;
            let g = g.trim();
            let required = match v.trim().to_lowercase().as_str() {
                "review" | "required" | "approve" => true,
                "direct" | "none" | "off" => false,
                _ => return None,
            };
            (!g.is_empty()).then(|| (g.to_string(), required))
        })
        .collect()
}

/// The rules as the field shows them.
#[must_use]
pub fn format_rules(rules: &[(String, bool)]) -> String {
    rules
        .iter()
        .map(|(g, r)| format!("{g}={}", if *r { "review" } else { "direct" }))
        .collect::<Vec<_>>()
        .join(", ")
}

struct Parts {
    head: Signal<String>,
    publish_note: Signal<String>,
    publish_bar: WidgetId,
    review_bar: WidgetId,
    rules_group: WidgetId,
    requests: WidgetId,
    required_label: Signal<String>,
    rules: Signal<String>,
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let f = frame(pb, forge_ui::tr!("Publish queue"))?;
        let root = f.root;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Heading)?;
        let pg = group(
            pb,
            root,
            "publish_group",
            forge_ui::tr!("Publish your sandbox"),
        )?;
        let publish_note = pb.b.signal(String::new());
        text(pb, pg, "publish_note", publish_note, LabelKind::Muted)?;
        let publish_bar = bar(pb, pg, "publish_bar", forge_ui::tr!("Publish"))?;
        let message = field(
            pb,
            publish_bar,
            "message",
            forge_ui::tr!("Message"),
            forge_ui::tr!("what changed (optional)"),
            "",
        )?;
        let publish = primary(pb, publish_bar, "publish", forge_ui::tr!("Publish"))?;
        let rg = group(pb, root, "requests_group", forge_ui::tr!("Review queue"))?;
        let requests = list(pb, rg, "requests", forge_ui::tr!("Publish requests"), 110.0)?;
        let review_bar = bar(pb, rg, "review_bar", forge_ui::tr!("Review"))?;
        let approve = primary(pb, review_bar, "approve", forge_ui::tr!("Approve"))?;
        let reason = field(
            pb,
            review_bar,
            "reason",
            forge_ui::tr!("Reason"),
            forge_ui::tr!("why not (for a rejection)"),
            "",
        )?;
        let reject = button(pb, review_bar, "reject", forge_ui::tr!("Reject"))?;
        let rules_group = group(pb, root, "rules_group", forge_ui::tr!("Review rules"))?;
        let required_label = pb.b.signal(String::new());
        text(pb, rules_group, "required", required_label, LabelKind::Body)?;
        let rules_bar = bar(pb, rules_group, "rules_bar", forge_ui::tr!("Review rules"))?;
        let toggle = button(
            pb,
            rules_bar,
            "toggle_required",
            forge_ui::tr!("Require approval: switch"),
        )?;
        let rules = field(
            pb,
            rules_group,
            "rules",
            forge_ui::tr!("Per-path rules"),
            forge_ui::tr!("scene/Sandbox/**=direct, engine-config/**=review"),
            "",
        )?;
        let save_rules = button(pb, rules_group, "save_rules", forge_ui::tr!("Save rules"))?;
        text(pb, root, "status", f.status, LabelKind::Muted)?;

        let rows: Rc<RefCell<Vec<Request>>> = Rc::default();
        let parts = Rc::new(Parts {
            head,
            publish_note,
            publish_bar,
            review_bar,
            rules_group,
            requests,
            required_label,
            rules,
        });

        pb.on(publish, move |act, _: &Pressed| {
            let m = message.get(act.ui.rt());
            act.cmd.emit(c::publish_command(m.trim()));
            message.set(act.ui.rt_mut(), String::new());
            act.want_turn();
        });
        {
            let rows = rows.clone();
            pb.on(approve, move |act, _: &Pressed| {
                let id = selected(act.ui, requests)
                    .and_then(|k| rows.borrow().get(k as usize).map(|r| r.id));
                if let Some(id) = id {
                    act.cmd.emit(c::approve_command(id));
                }
                act.want_turn();
            });
        }
        {
            let rows = rows.clone();
            pb.on(reject, move |act, _: &Pressed| {
                let id = selected(act.ui, requests)
                    .and_then(|k| rows.borrow().get(k as usize).map(|r| r.id));
                if let Some(id) = id {
                    let why = reason.get(act.ui.rt());
                    act.cmd.emit(c::reject_command(
                        id,
                        if why.trim().is_empty() {
                            forge_ui::tr!("no reason given")
                        } else {
                            why.trim()
                        },
                    ));
                    reason.set(act.ui.rt_mut(), String::new());
                }
                act.want_turn();
            });
        }
        pb.on(toggle, move |act, _: &Pressed| {
            let (required, current) = c::review_rules(act.mirror.settings());
            act.cmd.emit(c::review_policy_command(!required, &current));
            act.want_turn();
        });
        pb.on(save_rules, move |act, _: &Pressed| {
            let (required, _) = c::review_rules(act.mirror.settings());
            let typed = parse_rules(&rules.get(act.ui.rt()));
            act.cmd.emit(c::review_policy_command(required, &typed));
            act.want_turn();
        });

        let mut feeds = false;
        let mut seen = u64::MAX;
        pb.sync(root, move |s| {
            feed_once(s, f.relay, &mut feeds, crate::LIVE_HZ);
            let rev = s.services.collab.revision() ^ s.mirror.revision().rotate_left(15);
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            let ok = notice(s.ui, s.services, &f);
            refresh(s.ui, s.services, s.mirror, &parts, &rows, ok);
            set(s.ui, f.status, last_outcome(s.services, TARGETS));
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}

fn refresh(
    ui: &mut Ui,
    services: &forge_editor::services::EditorServices,
    mirror: &forge_editor::mirror::ProjectMirror,
    p: &Parts,
    rows: &Rc<RefCell<Vec<Request>>>,
    ok: bool,
) {
    let collab = &services.collab;
    let me = collab.me();
    let role = collab.my_role();
    let joined = collab.status().is_some_and(|s| s.joined);
    let requests = collab
        .server
        .as_ref()
        .map(|s| s.requests())
        .unwrap_or_default();
    let pending = requests
        .iter()
        .filter(|r| r.state == RequestState::Pending)
        .count();
    set(
        ui,
        p.head,
        forge_ui::trf!("{pending} request(s) waiting for review", pending),
    );
    let (required, rules) = c::review_rules(mirror.settings());
    let scopes = collab
        .team()
        .and_then(|t| t.member(&me).map(|m| m.paths.clone()))
        .unwrap_or_default();
    set(
        ui,
        p.publish_note,
        match role {
            None => forge_ui::tr!("Join a team to publish.").into(),
            Some(r) if !r.edits() => {
                forge_ui::trf!(
                    "A {label} does not publish.",
                    label = forge_ui::l10n::tr(r.label())
                )
            }
            Some(r) if r.maintains() => {
                forge_ui::tr!("You publish straight to the team baseline.").into()
            }
            Some(_) => format!(
                "{}{}",
                if required || rules.iter().any(|(_, r)| *r) {
                    forge_ui::tr!(
                        "Your publish waits for a reviewer's approval where the rules ask for it."
                    )
                } else {
                    forge_ui::tr!("You publish straight to the team baseline.")
                },
                if scopes.is_empty() {
                    String::new()
                } else {
                    forge_ui::trf!(" Your scope: {items}.", items = scopes.join(", "))
                }
            ),
        },
    );
    show(
        ui,
        p.publish_bar,
        ok && joined && role.is_some_and(Role::edits),
    );
    show(
        ui,
        p.review_bar,
        ok && joined && role.is_some_and(Role::approves),
    );
    show(
        ui,
        p.rules_group,
        joined && role.is_some_and(Role::maintains),
    );
    set(
        ui,
        p.required_label,
        forge_ui::trf!(
            "Approval is {required} for every publish (by a Developer){per_path}.",
            required = if required {
                forge_ui::tr!("required")
            } else {
                forge_ui::tr!("not required")
            },
            per_path = if rules.is_empty() {
                String::new()
            } else {
                forge_ui::trf!(
                    "; per path: {format_rules}",
                    format_rules = format_rules(&rules)
                )
            }
        ),
    );
    if p.rules.get(ui.rt()).is_empty() && !rules.is_empty() {
        set(ui, p.rules, format_rules(&rules));
    }
    // Newest first.
    let mut shown = requests;
    shown.reverse();
    fill(
        ui,
        p.requests,
        shown
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    i as u64,
                    format!(
                        "#{} {} \u{2014} {} \u{2014} \u{201c}{}\u{201d} \u{2014} {}",
                        r.id,
                        r.author,
                        state_label(&r.state),
                        r.message,
                        if r.paths.is_empty() {
                            forge_ui::trf!("{n} command(s)", n = r.commands)
                        } else {
                            r.paths.join(", ")
                        }
                    ),
                    r.state != RequestState::Pending || !ok,
                )
            })
            .collect(),
    );
    *rows.borrow_mut() = shown;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_round_trip_through_the_field() {
        let r = parse_rules("scene/Sandbox/**=direct, engine-config/**=review, junk");
        assert_eq!(
            r,
            vec![
                ("scene/Sandbox/**".to_string(), false),
                ("engine-config/**".to_string(), true)
            ]
        );
        assert_eq!(parse_rules(&format_rules(&r)), r);
    }
}
