//! **Build and export** (`forge.export`, Ch.27, Ch.38 §38.5, §38.7, DoD M2-51).
//!
//! Targets (Windows and Linux supported; Web and Android best-effort), and — before
//! anything is built — exactly the engine credit the packager will insert (E-64): the
//! credits entry, the executable metadata and where it goes on that target, `forge.json`,
//! the `NOTICES` line, the licensee's store-page part, and the default splash. A preview
//! of the `NOTICES` file follows. Building is `forge.export.build`, a command the core
//! performs through its `Packager` (in-memory until the HAL exporters, M8: it produces the
//! attribution, `forge.json`, credits and `NOTICES`, and lists the executable as UNBUILT).

use std::cell::Cell;
use std::rc::Rc;

use forge_editor::panels::PanelCx;
use forge_editor::project::{BUILD_CMD, ProjectOp};
use forge_project::packager::{Product, Support, Target, attribution, notices_preview};
use forge_ui::widgets::{Button, LabelKind, Pressed, RowItem, SelectionChanged, VirtualTree};
use forge_ui::{NodeStyle, Role};

use crate::ui::{bar, group, set, text};

/// Lines of `NOTICES` the preview shows.
pub const NOTICES_LINES: usize = 40;

fn target_label(t: Target) -> String {
    format!(
        "{} \u{2014} {}",
        forge_ui::l10n::tr_str(t.label()),
        match t.support() {
            Support::Supported => forge_ui::tr!("supported"),
            Support::BestEffort => forge_ui::tr!("best effort"),
        }
    )
}

/// The attribution preview's text for `product` on `target` (what the panel shows).
pub fn attribution_text(product: &Product, target: Target) -> Vec<(String, String)> {
    let a = attribution(product, target);
    vec![
        (
            forge_ui::tr!("In-product credits").into(),
            a.credits.clone(),
        ),
        (
            forge_ui::tr!("Executable metadata").into(),
            format!(
                "{} \u{2014} {}",
                a.carrier,
                a.metadata
                    .iter()
                    .map(|(k, v)| format!("{k} = {v}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        ),
        ("forge.json".into(), a.forge_json.clone()),
        ("NOTICES".into(), a.notices.clone()),
        (forge_ui::tr!("Store page").into(), a.store_page.clone()),
        (
            forge_ui::tr!("Splash").into(),
            if a.splash_default_on {
                forge_ui::tr!("\u{201c}Made with Forge Engine\u{201d} splash: on by default").into()
            } else {
                forge_ui::tr!("No splash").into()
            },
        ),
    ]
}

fn product_of(m: &forge_editor::mirror::ProjectMirror) -> Product {
    let text = |k: &str, d: &str| match m.setting(k) {
        Some(forge_cmd::Value::Text(t)) if !t.trim().is_empty() => t.clone(),
        _ => d.to_string(),
    };
    Product {
        name: text(forge_project::NAME_SETTING, forge_ui::tr!("Untitled")),
        version: text(forge_project::VERSION_SETTING, "0.1.0"),
        gpu_mode: forge_project::gpu_mode_id(match m.setting(forge_project::GPU_MODE_SETTING) {
            Some(forge_cmd::Value::Text(t)) => Some(t.as_str()),
            _ => None,
        })
        .into(),
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("the build targets and the NOTICES preview"));
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let root = pb.b.add(
            pb.parent,
            "content",
            NodeStyle::column(space[1])
                .padding(space[2])
                .grow(1.0)
                .scrollable(),
            forge_ui::widgets::Container::new(Role::Group).labelled(forge_ui::tr!("Build and export")),
        )?;
        text(pb, root, "title", forge_ui::tr!("Build and export"), LabelKind::Heading)?;
        let mut t = VirtualTree::list(forge_ui::tr!("Targets")).single_select();
        for (i, tg) in Target::ALL.iter().enumerate() {
            t.push(None, i as u64, RowItem::new(target_label(*tg)));
        }
        t.select(&[0]);
        let targets = pb.b.add(root, "targets", NodeStyle::leaf().height(100.0), t)?;
        let bbar = bar(pb, root, "build_bar", forge_ui::tr!("Build"))?;
        let build_label = pb.b.signal(String::from(forge_ui::tr!("Build")));
        let build = pb.b.add(
            bbar,
            "build",
            NodeStyle::leaf(),
            Button::new(build_label).primary(),
        )?;

        let ag = group(pb, root, "attribution", forge_ui::tr!("The engine credit this build carries (E-64)"))?;
        text(
            pb,
            ag,
            "attribution_hint",
            forge_ui::tr!("Every product made with Forge Engine credits it. The packager inserts the first four automatically; the credit is static text and metadata \u{2014} nothing in it runs, reports or connects."),
            LabelKind::Muted,
        )?;
        let rows: Vec<forge_ui::Signal<String>> = (0..6).map(|_| pb.b.signal(String::new())).collect();
        for (i, s) in rows.iter().enumerate() {
            // "Label: value": the value is the text the build writes (the credit, the metadata,
            // forge.json, the NOTICES line), shown exactly as written.
            crate::ui::verbatim_text(pb, ag, forge_ui::Key::Str(format!("a{i}").into()), *s, LabelKind::Body)?;
        }

        let ng = group(pb, root, "notices_group", forge_ui::tr!("NOTICES preview"))?;
        let (head, total) = notices_preview(NOTICES_LINES);
        crate::ui::verbatim_text(pb, ng, "notices", head, LabelKind::Mono)?;
        text(
            pb,
            ng,
            "notices_more",
            forge_ui::trf!("\u{2026} {more} more line(s): the full file ships beside the executable.", more = total.saturating_sub(NOTICES_LINES)),
            LabelKind::Muted,
        )?;

        let result = pb.b.signal(String::new());
        text(pb, root, "result", result, LabelKind::Body)?;
        let backend = pb.b.signal(String::new());
        text(pb, root, "backend", backend, LabelKind::Small)?;

        let target = Rc::new(Cell::new(Target::Windows));
        let rows2 = rows.clone();
        {
            let target = target.clone();
            pb.on(targets, move |act, e: &SelectionChanged| {
                if let Some(t) = e.keys.first().and_then(|k| Target::ALL.get(*k as usize)) {
                    target.set(*t);
                }
                let p = product_of(act.mirror);
                for ((label, v), s) in attribution_text(&p, target.get()).into_iter().zip(&rows) {
                    set(act.ui, *s, forge_ui::trf!("{label}: {value}", label = forge_ui::l10n::tr_str(&label), value = v));
                }
                set(act.ui, build_label, forge_ui::trf!("Build for {target}", target = forge_ui::l10n::tr_str(target.get().label())));
            });
        }
        {
            let target = target.clone();
            pb.on(build, move |act, _: &Pressed| {
                act.cmd.emit(
                    ProjectOp::Build {
                        target: target.get(),
                    }
                    .command(),
                );
            });
        }
        let mut seen = (u64::MAX, u64::MAX);
        let mut seen_outcome = pb
            .mirror()
            .project()
            .and_then(|s| s.outcomes.last().map(|o| o.id))
            .unwrap_or(0);
        pb.sync(root, move |s| {
            let rev = (s.mirror.settings_revision(), s.mirror.project_revision());
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            let p = product_of(s.mirror);
            for ((label, v), sig) in attribution_text(&p, target.get()).into_iter().zip(&rows2) {
                set(s.ui, *sig, forge_ui::trf!("{label}: {value}", label = forge_ui::l10n::tr_str(&label), value = v));
            }
            set(s.ui, build_label, forge_ui::trf!("Build for {target}", target = forge_ui::l10n::tr_str(target.get().label())));
            if let Some(st) = s.mirror.project() {
                set(
                    s.ui,
                    backend,
                    if st.packager.1 {
                        forge_ui::trf!("Packager: {packager} (in memory)", packager = forge_ui::l10n::tr_str(&st.packager.0))
                    } else {
                        forge_ui::trf!("Packager: {packager}", packager = forge_ui::l10n::tr_str(&st.packager.0))
                    },
                );
                let from = seen_outcome;
                for o in st.outcomes.iter().filter(|o| o.id > from) {
                    seen_outcome = o.id;
                    if o.op != BUILD_CMD {
                        continue;
                    }
                    let line = match (&o.result, &st.last_build) {
                        (Ok(m), Some(b)) => forge_ui::trf!(
                            "{m}. Files: {files}",
                            m,
                            files = b
                                .files
                                .iter()
                                .map(|f| match &f.unbuilt {
                                    None => format!("{} ({} B)", f.path, f.bytes),
                                    Some(_) => {
                                        forge_ui::trf!("{path} (UNBUILT)", path = f.path)
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        (Ok(m), None) => m.clone(),
                        (Err((c, m)), _) => forge_ui::trf!("Build failed ({c}): {m}", c, m),
                    };
                    set(s.ui, result, line);
                }
            }
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}
