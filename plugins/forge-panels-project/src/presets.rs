//! The **preset switcher and promotion dialog** (`forge.presets`, Ch.31 §31.4, DoD M2-48).
//!
//! Every arrow between the editor's presets is offered: 2D, 3D and every preset a plugin's
//! promotion rule adds (`forge_editor::project::promote::PromotionRule`). Picking a preset
//! shows the arrow's cost in the words of Ch.31 §31.4 and, computed from *this* project,
//! exactly what would be lost (how many entities, which ones). A lossless switch is one
//! click. A lossy one **states the loss and asks for confirmation** before anything is sent
//! (`test_lossy_warnings_present`); the switch is then `forge.project.promote` with
//! `"accept_loss": true` — one undoable command. Without that flag the core refuses a
//! lossy promotion, so no client can lose data by accident.
//!
//! **The presets are the project's (WP-16).** A project may carry its own copy of a preset
//! (`presets/3d/…` in its folder: "users author their own presets by copying one"); that copy
//! is what 3D means for this project — its defaults are what a switch applies, and its row
//! says so. The panel reads the copies from the project status (once per load) and plans
//! with them exactly as the core's promote command does.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use forge_cmd::Value;
use forge_editor::panels::PanelCx;
use forge_editor::presets::{PresetSet, PresetSource};
use forge_editor::project::promote::{
    MirrorView, Promotion, PromotionRules, plan_promotion_to, preset_dir_of, promote_command_to,
};
use forge_project::PRESET_SETTING;
use forge_ui::widgets::{Button, LabelKind, Pressed, RowItem, SelectionChanged, VirtualTree};
use forge_ui::{NodeStyle, Role, Ui, WidgetId};

use crate::ui::{Follow, bar, button, group, set, show, text};

/// The presets the open project works with, as the panel plans with them.
#[derive(Default)]
struct Presets {
    /// The status epoch and catalog revision they were read at (presets change only with a
    /// load or a preset plugin installed).
    seen: Option<(Option<u64>, u64)>,
    /// Directories the project replaced with its own copy.
    own: BTreeSet<String>,
    /// Every preset's defaults, by directory.
    defaults: BTreeMap<String, BTreeMap<String, Value>>,
    /// Keys the presets give defaults beyond the built-in rule keys.
    extra: BTreeSet<String>,
    problems: Vec<String>,
}

#[derive(Default)]
struct State {
    /// The preset directory picked.
    target: Option<String>,
    plan: Option<Promotion>,
    /// The preset last shown.
    current: Option<String>,
    /// The project as the rules see it, followed only while a plan is wanted.
    view: MirrorView,
    presets: Presets,
}

fn row_label(label: &str, this: bool, own: bool) -> String {
    let base = if this {
        forge_ui::trf!("{preset} (this project)", preset = label)
    } else {
        label.to_string()
    };
    if own {
        forge_ui::trf!("{base} \u{b7} the project's own preset", base)
    } else {
        base
    }
}

/// Read the project's presets when a load (or a preset plugin) changed them; whether
/// anything changed.
fn follow_presets(
    st: &mut State,
    m: &forge_editor::mirror::ProjectMirror,
    services: &forge_editor::services::EditorServices,
) -> bool {
    let epoch = m.project().map(|s| s.epoch);
    let shared = services.hosting.borrow().presets().clone();
    let catalog = shared
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let seen = (epoch, catalog.revision());
    if st.presets.seen == Some(seen) && epoch.is_some() {
        return false;
    }
    let files = m
        .project()
        .and_then(|s| s.open.as_ref())
        .map(|o| o.presets.clone())
        .unwrap_or_default();
    let mut p = Presets {
        seen: Some(seen),
        ..Presets::default()
    };
    if let Ok(set) = PresetSet::with_project_in(&catalog, &files) {
        p.own = set
            .iter()
            .filter(|(_, _, s)| *s == PresetSource::Project)
            .map(|(d, _, _)| d.to_string())
            .collect();
        p.defaults = set.defaults();
        let rule = forge_editor::project::promote::rule_keys();
        p.extra = p
            .defaults
            .values()
            .flat_map(|d| d.keys())
            .filter(|k| !rule.contains(*k))
            .cloned()
            .collect();
        p.problems = set.problems;
    }
    let changed = p.own != st.presets.own || p.defaults != st.presets.defaults;
    st.presets = p;
    changed
}

/// The detail lines for `p`: the arrow's cost and what this project would lose.
pub fn describe(p: &Promotion) -> (String, String) {
    let cost = p.describe.join(" ");
    let losses = if p.lossy() {
        forge_ui::trf!(
            "\u{26a0} Lossy for this project: {losses}.",
            losses = p
                .losses
                .iter()
                .map(|l| l.line())
                .collect::<Vec<_>>()
                .join("; ")
        )
    } else {
        forge_ui::tr!("Nothing in this project is lost.").to_string()
    };
    (cost, losses)
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("the built-in presets"));
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let fault_skip = pb.services().faults.lossy_warning_skipped();
        let services = pb.services();
        let rules: Rc<PromotionRules> = Rc::clone(&services.promotion_rules);
        // The presets offered: 2D, 3D, then the rules' (their order is stable for a run).
        let dirs: Vec<String> = rules.dirs();
        let root = pb.b.add(
            pb.parent,
            "content",
            NodeStyle::column(space[1])
                .padding(space[2])
                .grow(1.0)
                .scrollable(),
            forge_ui::widgets::Container::new(Role::Group).labelled(forge_ui::tr!("Presets")),
        )?;
        let current = pb.b.signal(String::new());
        text(pb, root, "current", current, LabelKind::Heading)?;
        text(
            pb,
            root,
            "hint",
            forge_ui::tr!("A preset sets defaults and never gates a feature: every panel and API works under every preset. Pick one to see what switching costs."),
            LabelKind::Muted,
        )?;
        let mut t = VirtualTree::list(forge_ui::tr!("Presets")).single_select();
        for (i, d) in dirs.iter().enumerate() {
            t.push(None, i as u64, RowItem::new(rules.label_of(d)));
        }
        let list =
            pb.b.add(root, "presets", NodeStyle::leaf().height(90.0), t)?;
        let detail = group(pb, root, "detail", forge_ui::tr!("What switching costs"))?;
        let cost = pb.b.signal(String::new());
        text(pb, detail, "cost", cost, LabelKind::Body)?;
        let losses = pb.b.signal(String::new());
        text(pb, detail, "losses", losses, LabelKind::Warning)?;
        let dbar = bar(pb, detail, "detail_bar", forge_ui::tr!("Preset actions"))?;
        let promote_label = pb.b.signal(String::from(forge_ui::tr!("Switch")));
        let promote = pb.b.add(
            dbar,
            "promote",
            NodeStyle::leaf(),
            Button::new(promote_label).primary(),
        )?;
        pb.b.hide(detail, true);
        let confirm = group(pb, root, "confirm", forge_ui::tr!("Confirm a lossy switch"))?;
        let confirm_text = pb.b.signal(String::new());
        text(pb, confirm, "confirm_text", confirm_text, LabelKind::Warning)?;
        let cbar = bar(pb, confirm, "confirm_bar", forge_ui::tr!("Confirm or cancel"))?;
        let yes = button(pb, cbar, "confirm_yes", forge_ui::tr!("Switch and accept the loss"))?;
        let no = button(pb, cbar, "confirm_no", forge_ui::tr!("Cancel"))?;
        pb.b.hide(confirm, true);
        text(
            pb,
            root,
            "backend",
            forge_ui::tr!("Presets are data: each is a manifest (presets/<name>/workspace.ron) that a built-in or a plugin's preset provides. A project that carries its own copy in its presets/ folder uses that copy."),
            LabelKind::Small,
        )?;
        let problems = pb.b.signal(String::new());
        let problems_id = text(pb, root, "preset_problems", problems, LabelKind::Warning)?;
        pb.b.hide(problems_id, true);

        let state = Rc::new(RefCell::new(State {
            view: MirrorView::tracking(rules.tracking().clone()),
            ..State::default()
        }));
        let follow = Follow::new(&pb.services().faults);
        // Follow the project and show it. The preset is one exact lookup; the entities are
        // followed (incrementally, through the mirror's change logs) only while a target is
        // picked, and planned over again only when something the dialog shows changed — so
        // an edit the dialog does not show (a painted tile chunk, a dragged mesh) costs
        // nothing more than reading the change.
        let refresh = {
            let (rules, dirs, services) = (Rc::clone(&rules), dirs.clone(), Rc::clone(&services));
            Rc::new(move |ui: &mut Ui,
                          m: &forge_editor::mirror::ProjectMirror,
                          st: &mut State,
                          list: WidgetId| {
                let here = preset_dir_of(m.setting(PRESET_SETTING)).to_string();
                let presets_moved = follow_presets(st, m, &services);
                if st.current.as_deref() != Some(here.as_str()) || presets_moved {
                    st.current = Some(here.clone());
                    set(
                        ui,
                        current,
                        forge_ui::trf!(
                            "This project uses the {label} preset.",
                            label = rules.label_of(&here)
                        ),
                    );
                    let own = st.presets.own.clone();
                    VirtualTree::edit(ui, list, |t| {
                        for (i, d) in dirs.iter().enumerate() {
                            let l = row_label(&rules.label_of(d), *d == here, own.contains(d));
                            if t.label_of(i as u64) != Some(l.as_str()) {
                                t.set_label(i as u64, &l);
                            }
                        }
                    });
                    show(ui, problems_id, !st.presets.problems.is_empty());
                    set(ui, problems, st.presets.problems.join(" "));
                    // The defaults a switch applies moved: plan again.
                    st.plan = None;
                }
                let wanted = st.target.clone().filter(|to| *to != here);
                if wanted.is_none() && !follow.always() {
                    st.plan = None;
                    show(ui, detail, false);
                    show(ui, confirm, false);
                    return;
                }
                let changed = follow.update(&mut st.view, m);
                let Some(to) = wanted else {
                    st.plan = None;
                    show(ui, detail, false);
                    show(ui, confirm, false);
                    return;
                };
                let stale = st
                    .plan
                    .as_ref()
                    .is_none_or(|p| p.from != here || p.to != to);
                if changed || stale {
                    let v = st.view.view();
                    follow.add(v.entities.len() + v.settings.len());
                    // The presets' defaults: also read the keys only they give defaults.
                    let mut v = v.clone();
                    for k in &st.presets.extra {
                        if let Some(val) = m.setting(k) {
                            v.settings.insert(k.clone(), val.clone());
                        }
                    }
                    let d = &st.presets.defaults;
                    let p = plan_promotion_to(
                        &v,
                        &to,
                        &|dir| d.get(dir).cloned().unwrap_or_default(),
                        &rules,
                    );
                    let (c, l) = describe(&p);
                    set(ui, cost, c);
                    set(ui, losses, l);
                    set(
                        ui,
                        promote_label,
                        forge_ui::trf!("Switch to {label}", label = p.to_label),
                    );
                    st.plan = Some(p);
                }
                show(ui, detail, true);
            })
        };
        {
            let state = state.clone();
            let refresh = refresh.clone();
            pb.on(list, move |act, e: &SelectionChanged| {
                let mut st = state.borrow_mut();
                st.target = e
                    .keys
                    .first()
                    .and_then(|k| dirs.get(*k as usize).cloned());
                show(act.ui, confirm, false);
                refresh(act.ui, act.mirror, &mut st, list);
            });
        }
        {
            let state = state.clone();
            let refresh = refresh.clone();
            pb.on(promote, move |act, _: &Pressed| {
                let mut st = state.borrow_mut();
                refresh(act.ui, act.mirror, &mut st, list);
                let Some(p) = st.plan.clone() else {
                    return;
                };
                if p.lossy() && !fault_skip {
                    set(
                        act.ui,
                        confirm_text,
                        forge_ui::trf!("\u{26a0} Switching {label} \u{2192} {label_2} loses data: {loss_text}. This is one undoable step. Continue?", label = p.from_label, label_2 = p.to_label, loss_text = p.loss_text()),
                    );
                    show(act.ui, confirm, true);
                } else {
                    // Lossless (or the W2 fault: the warning path removed).
                    act.cmd.emit(promote_command_to(&p.to, p.lossy()));
                }
            });
        }
        {
            let state = state.clone();
            pb.on(yes, move |act, _: &Pressed| {
                if let Some(p) = state.borrow().plan.clone() {
                    act.cmd.emit(promote_command_to(&p.to, true));
                }
                show(act.ui, confirm, false);
            });
        }
        pb.on(no, move |act, _: &Pressed| show(act.ui, confirm, false));
        let mut seen = u64::MAX;
        pb.sync(root, move |s| {
            let rev = s.mirror.revision();
            if rev != seen {
                seen = rev;
                refresh(s.ui, s.mirror, &mut state.borrow_mut(), list);
            }
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}
