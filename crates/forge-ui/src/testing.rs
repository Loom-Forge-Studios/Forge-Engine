//! The headless test harness (Ch.21 §21.13, §21.22).
//!
//! [`Harness`] owns a [`Ui`], a [`RecordingRenderer`] and an **injected clock**, and runs
//! the loop exactly as the winit runner does: it sleeps whenever [`Ui::next_wake`] says
//! `Wait`, wakes only for timer/animation deadlines and waker calls, and counts every
//! wakeup and every rendered frame. A 10 s idle therefore takes no wall-clock time, and
//! "0 frames, 0 wakeups" is a deterministic, machine-independent assertion.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::UiError;
use crate::damage::{UiWaker, Wake};
use crate::geom::Point;
use crate::id::WidgetId;
use crate::input::{ImeEvent, InputEvent, KeyCode, KeyEvent, Modifiers, PointerButton};
use crate::render::{FrameStats, RecordingRenderer, TargetId, UiRenderer};
use crate::ui::{Ui, UiConfig};

/// Counts wake requests from other threads (the stand-in for `EventLoopProxy`).
#[derive(Default)]
pub struct CountingWaker {
    pub wakes: AtomicU64,
}

impl UiWaker for CountingWaker {
    fn wake(&self) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
    }
}

/// The loop, headless.
pub struct Harness {
    pub ui: Ui,
    pub renderer: RecordingRenderer,
    pub waker: Arc<CountingWaker>,
    /// Loop iterations caused by something other than direct input: timers, animation
    /// deadlines, and waker calls.
    pub wakeups: u64,
    /// Frames actually rendered (damage was non-empty).
    pub frames: u64,
    seen_wakes: u64,
    now: Duration,
    target: TargetId,
}

impl Harness {
    pub fn new(cfg: UiConfig) -> Result<Harness, UiError> {
        let mut ui = Ui::new(cfg)?;
        let waker = Arc::new(CountingWaker::default());
        ui.set_waker(waker.clone());
        Ok(Harness {
            ui,
            renderer: RecordingRenderer::new(),
            waker,
            wakeups: 0,
            frames: 0,
            seen_wakes: 0,
            now: Duration::ZERO,
            target: TargetId(0),
        })
    }

    pub fn now(&self) -> Duration {
        self.now
    }

    /// Move the injected clock forward to `t` (never backwards). An application loop
    /// built on the harness (the editor shell's) uses it to wake at its own deadlines.
    pub fn set_now(&mut self, t: Duration) {
        self.now = self.now.max(t);
    }

    /// Whether a waker was called since the last check (and count it as a wakeup).
    pub fn take_wake(&mut self) -> bool {
        let woke = self.waker.wakes.load(Ordering::SeqCst);
        if woke != self.seen_wakes {
            self.seen_wakes = woke;
            self.wakeups += 1;
            return true;
        }
        false
    }

    /// Whether a waker was called that the loop has not seen yet (not consumed, not counted).
    pub fn wake_pending(&self) -> bool {
        self.waker.wakes.load(Ordering::SeqCst) != self.seen_wakes
    }

    /// One loop iteration at the current time: frame, then render if damaged.
    pub fn step(&mut self) -> Option<FrameStats> {
        self.ui.frame(self.now);
        match self.ui.render(&mut self.renderer, self.target) {
            Ok(Some(s)) => {
                self.frames += 1;
                Some(s)
            }
            _ => None,
        }
    }

    /// Run iterations until the UI settles (no damage pending at the current time).
    pub fn settle(&mut self) {
        for _ in 0..16 {
            self.step();
            if !self.ui.has_damage() {
                let w = self.ui.next_wake();
                if !matches!(w, Wake::WaitUntil(t) if t <= self.now) {
                    return;
                }
            }
        }
    }

    /// Advance the injected clock by `d`, waking exactly when the loop would: at each
    /// timer/animation deadline, and whenever a waker was called. Returns the frames
    /// rendered during the interval.
    pub fn advance(&mut self, d: Duration) -> u64 {
        let end = self.now + d;
        let start_frames = self.frames;
        // Iterations are bounded by construction (each wakes for a real deadline); the
        // cap only guards a runaway animation from hanging a test.
        for _ in 0..1_000_000 {
            let woke = self.waker.wakes.load(Ordering::SeqCst);
            if woke != self.seen_wakes {
                self.seen_wakes = woke;
                self.wakeups += 1;
                self.step();
                continue;
            }
            match self.ui.next_wake() {
                Wake::Now => {
                    self.step();
                }
                Wake::WaitUntil(t) if t <= end => {
                    self.now = self.now.max(t);
                    self.wakeups += 1;
                    self.step();
                }
                Wake::WaitUntil(_) | Wake::Wait => break,
            }
        }
        self.now = end;
        self.frames - start_frames
    }

    // ---- input helpers --------------------------------------------------------------

    pub fn input(&mut self, ev: InputEvent) {
        self.ui.handle(ev);
    }

    pub fn key(&mut self, code: KeyCode, mods: Modifiers) {
        self.ui.handle(InputEvent::Key(KeyEvent::press(code, mods)));
    }

    pub fn tab(&mut self, backward: bool) {
        self.key(
            KeyCode::Tab,
            if backward {
                Modifiers::SHIFT
            } else {
                Modifiers::NONE
            },
        );
    }

    /// Type text as individual key presses.
    pub fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            let code = if c == ' ' {
                KeyCode::Space
            } else {
                KeyCode::Char(c.to_ascii_lowercase())
            };
            let mut ev = KeyEvent::press(code, Modifiers::NONE);
            ev.text = Some(c.to_string());
            self.ui.handle(InputEvent::Key(ev));
        }
    }

    pub fn ime(&mut self, e: ImeEvent) {
        self.ui.handle(InputEvent::Ime(e));
    }

    pub fn move_to(&mut self, p: Point) {
        self.ui.handle(InputEvent::PointerMoved(p));
    }

    /// Click the centre of a widget.
    pub fn click(&mut self, id: WidgetId) {
        let Some(r) = self.ui.rect(id) else { return };
        let p = r.center();
        self.ui.handle(InputEvent::PointerMoved(p));
        self.ui.handle(InputEvent::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed: true,
        });
        self.ui.handle(InputEvent::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed: false,
        });
    }

    /// Render one full frame (everything damaged) and return its stats.
    pub fn full_frame(&mut self) -> Option<FrameStats> {
        self.ui.damage_all();
        self.step()
    }

    pub fn renderer_mut(&mut self) -> &mut dyn UiRenderer {
        &mut self.renderer
    }
}

// ---- widget-test helpers (WP-U2 catalogue tests) -------------------------------------------

impl Harness {
    /// A harness holding one widget under a padded column, settled, with the accessibility
    /// tree active (so `node` reads what a screen reader would get).
    pub fn with_widget(
        widget: impl crate::Widget,
        style: crate::layout::NodeStyle,
    ) -> Result<(Harness, WidgetId), UiError> {
        let mut h = Harness::new(UiConfig::default())?;
        let host = h.ui.add(
            h.ui.root(),
            "host",
            crate::layout::NodeStyle::column(8.0).fill().padding(16.0),
            crate::widgets::Container::group(),
        )?;
        let id = h.ui.add(host, "w", style, widget)?;
        h.settle();
        h.ui.a11y_activate();
        Ok((h, id))
    }

    /// An empty host column to build composites into.
    pub fn with_host() -> Result<(Harness, WidgetId), UiError> {
        let mut h = Harness::new(UiConfig::default())?;
        let host = h.ui.add(
            h.ui.root(),
            "host",
            crate::layout::NodeStyle::column(8.0).fill().padding(16.0),
            crate::widgets::Container::group(),
        )?;
        Ok((h, host))
    }

    /// Focus a widget by keyboard.
    pub fn focus(&mut self, id: WidgetId) {
        self.ui.set_focus(Some(id), true);
        self.settle();
    }

    /// Press a key (with modifiers) and settle.
    pub fn press(&mut self, code: KeyCode, mods: Modifiers) {
        self.key(code, mods);
        self.settle();
    }

    /// The accessibility node last sent for a widget (activate first).
    pub fn node(&mut self, id: WidgetId) -> Option<accesskit::Node> {
        if !self.ui.a11y_active() {
            self.ui.a11y_activate();
        }
        self.settle();
        self.ui.a11y_node(id).cloned()
    }

    /// A virtual accessibility child (list row, curve key, tab) of `id`.
    pub fn child_node(&mut self, id: WidgetId, local: u64) -> Option<accesskit::Node> {
        self.node(id.child(&crate::id::Key::Id(local)))
    }

    /// Actions of type `A` raised since the last call (others are dropped).
    pub fn take<A: Clone + 'static>(&mut self) -> Vec<A> {
        self.ui
            .take_actions()
            .iter()
            .filter_map(|a| a.get::<A>().cloned())
            .collect()
    }

    /// Press the primary button at `p` and release it there.
    pub fn click_at(&mut self, p: Point) {
        self.ui.handle(InputEvent::PointerMoved(p));
        self.ui.handle(InputEvent::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed: true,
        });
        self.ui.handle(InputEvent::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed: false,
        });
        self.settle();
    }

    /// Drag with the primary button from `a` to `b` in `steps` moves.
    pub fn drag(&mut self, a: Point, b: Point, steps: u32) {
        self.ui.handle(InputEvent::PointerMoved(a));
        self.ui.handle(InputEvent::PointerButton {
            pos: a,
            button: PointerButton::Primary,
            pressed: true,
        });
        for i in 1..=steps.max(1) {
            let t = i as f32 / steps.max(1) as f32;
            self.ui.handle(InputEvent::PointerMoved(Point::new(
                a.x + (b.x - a.x) * t,
                a.y + (b.y - a.y) * t,
            )));
        }
        self.ui.handle(InputEvent::PointerButton {
            pos: b,
            button: PointerButton::Primary,
            pressed: false,
        });
        self.settle();
    }
}

// ---- timed gates beside background work ---------------------------------------------------

// Every wall-clock gate of the workspace — these UI gates, the perf gate, the streaming walk,
// the store recorders — runs its timed body through one helper,
// `forge_trace::timed` (WP-19, moved there from here so lane-A crates below the UI share it):
// a child process running only that test, at the High priority class (read back), holding a
// machine-wide lock so no two timed bodies (of any lane, either runner) measure at once. See
// that module for why each part is needed. Scheduling, not a tolerance (W5).
//
// Reentrancy guard: `run_timed_alone` installs a per-thread call counter in the parent
// (private to forge-trace). Calling it a second time from the same test thread panics with
// the documented message — a positive control in this file's test module proves the panic
// fires. Without this guard, a double-call would silently return the first body's result,
// hiding the second body's failure (W5: no budget widened).
pub use forge_trace::timed::{
    TIMED_ALONE_ENV, priority_class, run_ahead_of_background_work, run_timed, run_timed_alone,
    set_priority_class,
};
