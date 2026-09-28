//! **Profiler** (`forge.profiler`, Ch.21 §21.21, DoD M2-40; Ch.29, Ch.26, Ch.25): a frame
//! timeline, the named budgets (red when over), the adapter pool's lanes and replication
//! statistics, read through the `CounterSource` trait.
//!
//! **A live-data panel (§21.11).** The view owns two feeds: the counter source (refreshed at
//! most [`PROFILER_HZ`] times a second, only while visible, only when its generation moved)
//! and the editor's own UI counters (`ui.self`) as a **self-UI** feed — shown, but it can
//! never wake the loop, so the profiler cannot keep the editor awake by measuring it
//! (`test_profiler_panel`). The editor's source is `forge-trace` (M2-11,
//! `forge_editor::sim_bridge::TraceCounters`); the panel names its source.

use std::sync::Arc;

use accesskit::Role;
use forge_editor::panels::PanelCx;
use forge_editor::profile::{CounterSource, ProfileSnapshot, SelfUiCounters};
use forge_ui::input::{Handled, UiEvent};
use forge_ui::text::TextStyle;
use forge_ui::widget::{A11yCx, EventCx, PaintCx, Widget};
use forge_ui::{ColorRole, LiveFeed, NodeStyle, Point, Rect};

/// The profiler refreshes at most this often (Ch.21 §21.21).
pub const PROFILER_HZ: u8 = 10;
/// The frame budget the timeline marks (60 Hz).
pub const FRAME_BUDGET_MS: f64 = 1000.0 / 60.0;

/// The profiler's view (see the module docs).
pub struct ProfilerView {
    source: Arc<dyn CounterSource>,
    ui_self: Arc<SelfUiCounters>,
    snap: ProfileSnapshot,
    /// Refreshes taken (tests read it).
    pub refreshes: u64,
}

impl ProfilerView {
    pub fn new(source: Arc<dyn CounterSource>, ui_self: Arc<SelfUiCounters>) -> Self {
        let snap = source.snapshot();
        Self {
            source,
            ui_self,
            snap,
            refreshes: 0,
        }
    }
    /// What the view shows now.
    pub fn snapshot(&self) -> &ProfileSnapshot {
        &self.snap
    }
    /// The names of the budgets shown over their allowance (drawn red).
    pub fn over_budget(&self) -> Vec<String> {
        self.snap
            .budgets
            .iter()
            .filter(|b| b.over())
            .map(|b| b.name.clone())
            .collect()
    }
}

impl Widget for ProfilerView {
    fn role(&self) -> Role {
        Role::Group
    }

    /// A keyboard user can move focus into the profiler, and a screen reader then reads its
    /// summary (frames, budgets, which are over). Hovering it re-records nothing.
    fn focusable(&self) -> bool {
        true
    }
    fn hover_sensitive(&self) -> bool {
        false
    }

    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::FeedChanged(_) => {
                let snap = self.source.snapshot();
                self.refreshes += 1;
                if snap != self.snap {
                    self.snap = snap;
                    cx.request_paint();
                    cx.request_a11y();
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }

    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.fill(r, ColorRole::BgBase, 0.0);
        let small = TextStyle::body(cx.theme().type_scale.small);
        let line = small.size * small.line_height;
        let pad = cx.theme().space[2];
        let mut y = r.y + pad;
        let x = r.x + pad;
        let w = (r.w - 2.0 * pad).max(0.0);
        let text = |cx: &mut PaintCx, s: &str, y: &mut f32, role: ColorRole| {
            cx.text(s, &small, Point::new(x, *y), role);
            *y += line;
        };
        text(
            cx,
            &forge_ui::trf!(
                "Source: {backend}",
                backend = forge_ui::l10n::tr_str(&self.source.backend())
            ),
            &mut y,
            ColorRole::FgMuted,
        );
        text(
            cx,
            &forge_ui::trf!(
                "UI (ui.self, never wakes): {frames} frames drawn \u{00b7} last frame {calls} draw calls",
                frames = self.ui_self.frames(),
                calls = self.ui_self.draw_calls()
            ),
            &mut y,
            ColorRole::FgMuted,
        );
        // ---- the frame timeline ----
        text(
            cx,
            forge_ui::tr!("Frame timeline (bars: ms; the line is the 16.7 ms frame)"),
            &mut y,
            ColorRole::FgPrimary,
        );
        let th = (line * 3.0).max(24.0);
        let area = Rect::new(x, y, w, th);
        cx.fill(area, ColorRole::BgSunken, 2.0);
        let frames = &self.snap.frames;
        if !frames.is_empty() {
            let n = frames.len().max(1);
            let bw = (w / n as f32).max(1.0);
            let scale = th as f64 / (2.0 * FRAME_BUDGET_MS);
            for (i, f) in frames.iter().enumerate() {
                let bh = ((f.total_ms * scale) as f32).clamp(1.0, th);
                let over = f.total_ms > FRAME_BUDGET_MS;
                cx.mark(
                    Rect::new(x + i as f32 * bw, y + th - bh, (bw - 1.0).max(1.0), bh),
                    if over {
                        ColorRole::Danger
                    } else {
                        ColorRole::Accent
                    },
                    ColorRole::BgSunken,
                    0.0,
                );
            }
            let budget_y = y + th - (FRAME_BUDGET_MS * scale) as f32;
            cx.mark(
                Rect::new(x, budget_y, w, 1.0),
                ColorRole::Warning,
                ColorRole::BgSunken,
                0.0,
            );
        }
        y += th + pad;
        // ---- named budgets ----
        text(
            cx,
            forge_ui::tr!("Named budgets (Ch.29; red when over)"),
            &mut y,
            ColorRole::FgPrimary,
        );
        if self.snap.budgets.is_empty() {
            text(
                cx,
                forge_ui::tr!("No budgets declared."),
                &mut y,
                ColorRole::FgMuted,
            );
        }
        for b in &self.snap.budgets {
            if y > r.bottom() - line {
                break;
            }
            let over = b.over();
            cx.mark(
                Rect::new(x, y + line * 0.25, line * 0.5, line * 0.5),
                if over {
                    ColorRole::Danger
                } else {
                    ColorRole::Success
                },
                ColorRole::BgBase,
                2.0,
            );
            let s = format!(
                "{}{}: {:.3} / {:.3} {}",
                if over { "OVER  " } else { "" },
                b.name,
                b.value,
                b.budget,
                b.unit
            );
            cx.text(
                &s,
                &small,
                Point::new(x + line * 0.75, y),
                if over {
                    ColorRole::Danger
                } else {
                    ColorRole::FgPrimary
                },
            );
            y += line;
        }
        y += pad;
        // ---- raw counters ----
        text(
            cx,
            forge_ui::tr!("Counters (raw values from the trace source)"),
            &mut y,
            ColorRole::FgPrimary,
        );
        let mut counters_sorted = self.snap.counters.clone();
        counters_sorted.sort_by(|a, b| a.0.cmp(&b.0));
        if counters_sorted.is_empty() {
            text(
                cx,
                forge_ui::tr!("No counters recorded."),
                &mut y,
                ColorRole::FgMuted,
            );
        }
        for (name, value) in &counters_sorted {
            if y > r.bottom() - line * 1.5 {
                break;
            }
            let s = format!("{name}: {value:.4}");
            text(cx, &s, &mut y, ColorRole::FgPrimary);
        }
        if self.snap.dropped_events > 0 {
            let dropped = self.snap.dropped_events;
            if y <= r.bottom() - line * 1.5 {
                text(
                    cx,
                    &forge_ui::trf!("WARNING: {dropped} trace events dropped", dropped),
                    &mut y,
                    ColorRole::Warning,
                );
            }
        }
        // ---- adapter pool lanes ----
        text(
            cx,
            forge_ui::tr!("Adapter pool lanes (Ch.26)"),
            &mut y,
            ColorRole::FgPrimary,
        );
        if self.snap.lanes.is_empty() {
            text(
                cx,
                forge_ui::tr!("No GPU work recorded yet."),
                &mut y,
                ColorRole::FgMuted,
            );
        }
        for lane in &self.snap.lanes {
            if y > r.bottom() - line * 2.0 {
                break;
            }
            text(cx, &lane.adapter, &mut y, ColorRole::FgMuted);
            let end = lane.spans.iter().map(|s| s.1).fold(1e-3, f64::max);
            let lh = line * 0.6;
            cx.fill(Rect::new(x, y, w, lh), ColorRole::BgSunken, 1.0);
            for (a, b, _) in &lane.spans {
                let sx = (a / end) as f32 * w;
                let sw = (((b - a) / end) as f32 * w).max(1.0);
                cx.mark(
                    Rect::new(x + sx, y, sw, lh),
                    ColorRole::Accent,
                    ColorRole::BgSunken,
                    0.0,
                );
            }
            y += lh + pad * 0.5;
        }
        y += pad;
        // ---- replication ----
        text(
            cx,
            forge_ui::tr!("Replication (Ch.25)"),
            &mut y,
            ColorRole::FgPrimary,
        );
        match &self.snap.replication {
            Some(rep) => text(
                cx,
                &forge_ui::trf!(
                    "{n} entities \u{00b7} in {kin} kB/s \u{00b7} out {kout} kB/s",
                    n = rep.entities,
                    kin = format!("{:.1}", rep.bytes_in_per_s / 1000.0),
                    kout = format!("{:.1}", rep.bytes_out_per_s / 1000.0)
                ),
                &mut y,
                ColorRole::FgPrimary,
            ),
            None => text(cx, &self.snap.replication_note, &mut y, ColorRole::FgMuted),
        }
    }

    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        let over = self.over_budget();
        node.set_label(forge_ui::trf!(
            "Profiler: {frames_count} frames, {budgets_count} budgets, {over_count} over{items}",
            frames_count = self.snap.frames.len(),
            budgets_count = self.snap.budgets.len(),
            over_count = over.len(),
            items = if over.is_empty() {
                String::new()
            } else {
                format!(" ({})", over.join(", "))
            }
        ));
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!(
        "the budgets of every subsystem, measured or not"
    ));
    cx.add_live(|pb| {
        let services = pb.services();
        let view = pb.b.add(
            pb.parent,
            "view",
            NodeStyle::default().grow(1.0),
            ProfilerView::new(
                Arc::clone(&services.profiler),
                Arc::clone(&services.ui_self),
            ),
        )?;
        // The feeds are registered on the view's first sync (the builder has no `Ui`).
        let mut registered = false;
        pb.sync(view, move |sy| {
            if !registered {
                registered = true;
                sy.ui.add_feed(
                    view,
                    LiveFeed {
                        source: services.profiler.clone() as Arc<dyn forge_ui::LiveSource>,
                        // W2 positive control only: an uncapped live panel.
                        max_hz: if services.faults.profiler_uncapped() {
                            60
                        } else {
                            PROFILER_HZ
                        },
                        self_ui: false,
                    },
                );
                sy.ui.add_feed(
                    view,
                    LiveFeed {
                        source: services.ui_self.cell(),
                        max_hz: PROFILER_HZ,
                        self_ui: true,
                    },
                );
            }
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}
