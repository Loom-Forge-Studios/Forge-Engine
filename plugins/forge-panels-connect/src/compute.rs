//! **Compute and farm** (`forge.compute`, Ch.21 §21.21, Ch.26; DoD M2-56): the adapters this
//! machine dispatches batch work to with their **measured** throughput, and the LAN farm's
//! nodes — discovery, status and failures — over the `ComputePool` trait.
//!
//! A live panel (§21.11): it refreshes at most twice a second, only while visible, only when
//! the pool's generation moved; an idle pool never wakes the editor.

use std::sync::Arc;

use forge_editor::connect::compute::ComputeSnapshot;
use forge_editor::panels::PanelCx;
use forge_ui::widgets::LabelKind;
use forge_ui::{LiveFeed, NodeStyle, Role, Ui, WidgetId};

use crate::relay::{FeedRelay, FeedTicked};
use crate::ui::{fill, group, list, set, text};

fn rate(v: Option<f64>) -> String {
    match v {
        None => forge_ui::tr!("not measured yet").into(),
        Some(r) if r >= 1e9 => forge_ui::trf!("{n} G items/s", n = format!("{:.2}", r / 1e9)),
        Some(r) if r >= 1e6 => forge_ui::trf!("{n} M items/s", n = format!("{:.2}", r / 1e6)),
        Some(r) if r >= 1e3 => forge_ui::trf!("{n} k items/s", n = format!("{:.1}", r / 1e3)),
        Some(r) => forge_ui::trf!("{n} items/s", n = format!("{r:.0}")),
    }
}

fn show(ui: &mut Ui, snap: &ComputeSnapshot, adapters: WidgetId, nodes: WidgetId) {
    fill(
        ui,
        adapters,
        snap.adapters
            .iter()
            .enumerate()
            .map(|(i, a)| {
                (
                    i as u64 + 1,
                    forge_ui::trf!(
                        "{name} ({kind}, {backend}) \u{b7} {rate} \u{b7} {n} job(s){excluded}",
                        name = a.name,
                        kind = a.kind,
                        backend = a.backend,
                        rate = rate(a.items_per_s),
                        n = a.jobs_done,
                        excluded = if a.enabled {
                            ""
                        } else {
                            forge_ui::tr!(" \u{b7} excluded")
                        }
                    ),
                    !a.enabled,
                    None,
                )
            })
            .collect(),
    );
    let mut rows = Vec::new();
    for (i, n) in snap.nodes.iter().enumerate() {
        let k = (i as u64 + 1) << 16;
        rows.push((
            k,
            format!(
                "{} at {} \u{b7} {} \u{b7} {}{}",
                n.id,
                n.host,
                n.status.label(),
                rate(n.items_per_s),
                if n.failures.is_empty() {
                    String::new()
                } else {
                    forge_ui::trf!(" \u{b7} {n} failure(s)", n = n.failures.len())
                }
            ),
            matches!(
                n.status,
                forge_editor::connect::compute::NodeStatus::Lost { .. }
            ),
            None,
        ));
        for (j, f) in n.failures.iter().enumerate() {
            rows.push((k | (j as u64 + 1), format!("\u{26a0} {f}"), false, Some(k)));
        }
    }
    fill(ui, nodes, rows);
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("this machine's adapters"));
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
            forge_ui::widgets::Container::new(Role::Group).labelled(forge_ui::tr!("Compute and farm")),
        )?;
        let relay = pb.b.add(root, "feed", NodeStyle::leaf(), FeedRelay::new())?;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Heading)?;
        let ag = group(pb, root, "adapters_group", forge_ui::tr!("Adapters (Tier 0: this machine)"))?;
        let adapters = list(pb, ag, "adapters", forge_ui::tr!("Compute adapters"), 110.0)?;
        text(
            pb,
            ag,
            "adapters_hint",
            forge_ui::tr!("Batch work is split by each adapter's measured throughput, not its name; results merge in a fixed order (Ch.26)."),
            LabelKind::Small,
        )?;
        let ng = group(pb, root, "nodes_group", forge_ui::tr!("LAN farm (Tier 2)"))?;
        let nodes = list(pb, ng, "nodes", forge_ui::tr!("Farm nodes"), 140.0)?;
        text(
            pb,
            root,
            "footer",
            forge_ui::trf!("Source: {backend}", backend = forge_ui::l10n::tr_str(&services.connect.compute.backend())),
            LabelKind::Small,
        )?;
        // The panel reads the pool only when the feed refreshed (so at most twice a second,
        // only while visible), never on an unrelated project change.
        let ticks = std::rc::Rc::new(std::cell::Cell::new(0u64));
        {
            let ticks = ticks.clone();
            pb.on(relay, move |act, _: &FeedTicked| {
                ticks.set(ticks.get() + 1);
                act.want_turn();
            });
        }

        let pool = Arc::clone(&services.connect.compute);
        let uncapped = services.connect.faults.compute_uncapped();
        let mut feed = false;
        let mut seen = u64::MAX;
        pb.sync(root, move |s| {
            if !feed {
                feed = true;
                s.ui.add_feed(
                    relay,
                    LiveFeed {
                        source: pool.clone() as Arc<dyn forge_ui::LiveSource>,
                        // W2 positive control only: an uncapped live panel.
                        max_hz: if uncapped { 60 } else { crate::LIVE_HZ },
                        self_ui: false,
                    },
                );
            }
            if ticks.get() == seen {
                return Ok(());
            }
            seen = ticks.get();
            let snap = pool.snapshot();
            let lost = snap
                .nodes
                .iter()
                .filter(|n| matches!(n.status, forge_editor::connect::compute::NodeStatus::Lost { .. }))
                .count();
            set(
                s.ui,
                head,
                forge_ui::trf!("{adapters_count} adapter(s) \u{b7} {nodes_count} farm node(s){dropped_note}{discovering}", adapters_count = snap.adapters.len(), nodes_count = snap.nodes.len(), dropped_note = if lost > 0 {
                        forge_ui::trf!(" \u{b7} {lost} lost", lost)
                    } else {
                        String::new()
                    }, discovering = if snap.discovering {
                        forge_ui::tr!(" \u{b7} discovering on the LAN")
                    } else {
                        ""
                    }),
            );
            show(s.ui, &snap, adapters, nodes);
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}
