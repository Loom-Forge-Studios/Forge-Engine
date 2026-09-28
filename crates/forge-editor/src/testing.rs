//! A headless editor loop for tests (the shell's twin of `forge_ui::testing::Harness`).
//!
//! [`Rig`] owns a core, a shell connected to it as a human, and a harness standing in for
//! the main OS window. [`Rig::turn`] is one iteration of the winit runner's loop for that
//! window — frame, the window's actions to the shell, the shell's per-window update, frame,
//! render if damaged — and [`Rig::advance`] runs the loop over injected time exactly as
//! the runner would: it wakes only for UI deadlines, the shell's own deadline (a debounced
//! save) and waker calls (another client changed the project), and counts every wakeup
//! and every rendered frame. So "idle is free" is a deterministic assertion.

use std::sync::Arc;
use std::time::Duration;

use forge_cmd::Issuer;
use forge_ui::damage::{UiWaker, Wake};
use forge_ui::testing::Harness;
use forge_ui::{InputEvent, KeyCode, KeyEvent, Modifiers, Size, UiConfig, UiError};

use crate::EditorError;
use crate::core::{EditorCore, LocalBus, SharedCore};
use crate::shell::{Shell, ShellConfig};

/// See the module docs.
pub struct Rig {
    pub h: Harness,
    pub shell: Shell,
    pub core: SharedCore,
}

impl Rig {
    /// A 1440x900 main window over a fresh core, the shell issuing as `human:tester`.
    pub fn new(cfg: ShellConfig) -> Result<Rig, EditorError> {
        Self::with_core(cfg, EditorCore::new())
    }

    pub fn with_core(cfg: ShellConfig, core: SharedCore) -> Result<Rig, EditorError> {
        Self::with_client(cfg, core, |waker| {
            let mut client = EditorCore::connect(
                &SharedCore::clone(&waker.1),
                Issuer::Human {
                    user: "tester".into(),
                },
            );
            client.set_waker(waker.0);
            Box::new(client)
        })
    }

    /// A rig whose shell reaches `core` through the client `connect` makes — the split
    /// editor's remote client over the wire (M2-16), where the core is in this process only
    /// because the test hosts it. `connect` gets the loop's waker (to call when answers
    /// arrive) and the core.
    pub fn with_client(
        cfg: ShellConfig,
        core: SharedCore,
        connect: impl FnOnce((crate::core::Waker, SharedCore)) -> Box<dyn crate::client::BusClient>,
    ) -> Result<Rig, EditorError> {
        let h = Harness::new(UiConfig {
            size: Size::new(1440.0, 900.0),
            ..UiConfig::default()
        })?;
        let w = h.waker.clone();
        let client = connect((Arc::new(move || w.wake()), core.clone()));
        let shell = Shell::new(cfg, client)?;
        let mut rig = Rig { h, shell, core };
        rig.shell.build_main(&mut rig.h.ui)?;
        rig.settle();
        Ok(rig)
    }

    /// Another client of the same core (an automation session, a script).
    pub fn connect(&self, issuer: Issuer) -> LocalBus {
        EditorCore::connect(&self.core, issuer)
    }

    fn route(&mut self) {
        let now = self.h.now();
        self.h.ui.frame(now);
        let a = self.h.ui.take_actions();
        if !a.is_empty() {
            self.shell.on_actions(&mut self.h.ui, a);
            self.h.ui.frame(now);
        }
    }

    /// One loop iteration (see the module docs).
    pub fn turn(&mut self) {
        self.route();
        self.shell.update_window(&mut self.h.ui);
        self.route();
        self.h.step();
    }

    /// Turn until nothing is left to do at the current time.
    pub fn settle(&mut self) {
        for _ in 0..24 {
            self.turn();
            // A lifecycle transfer (push, pull, clone, sign-in) runs on its own thread; a
            // test settles once it has ended and its outcome is shown.
            if EditorCore::wait_transfers(&self.core) {
                continue;
            }
            let due_now = matches!(self.h.ui.next_wake(), Wake::Now)
                || matches!(self.h.ui.next_wake(), Wake::WaitUntil(t) if t <= self.h.now())
                || self
                    .shell
                    .next_deadline()
                    .is_some_and(|d| d <= self.h.now());
            if !self.h.ui.has_damage() && !due_now {
                return;
            }
        }
    }

    /// Run the loop for `d` of injected time (see the module docs). Returns
    /// `(frames rendered, wakeups)` during the interval.
    pub fn advance(&mut self, d: Duration) -> (u64, u64) {
        let end = self.h.now() + d;
        let (f0, w0) = (self.h.frames, self.h.wakeups);
        for _ in 0..1_000_000 {
            if self.h.take_wake() {
                self.turn();
                continue;
            }
            let ui = match self.h.ui.next_wake() {
                Wake::Now => Some(self.h.now()),
                Wake::WaitUntil(t) => Some(t),
                Wake::Wait => None,
            };
            let next = match (ui, self.shell.next_deadline()) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            match next {
                Some(t) if t <= end => {
                    // Sleeping until a later deadline and waking for it is a wakeup; work
                    // already due now (pending damage) is the same turn continuing.
                    if t > self.h.now() {
                        self.h.wakeups += 1;
                    }
                    self.h.set_now(t);
                    self.turn();
                }
                _ => break,
            }
        }
        self.h.set_now(end);
        (self.h.frames - f0, self.h.wakeups - w0)
    }

    /// Press a chord in the main window (`Ctrl+Z`) and run one turn.
    pub fn chord(&mut self, code: KeyCode, mods: Modifiers) {
        self.h
            .ui
            .handle(InputEvent::Key(KeyEvent::press(code, mods)));
        self.turn();
    }

    /// Run an action by id, then one turn.
    pub fn run(&mut self, action: &str) {
        self.shell.run_action(&mut self.h.ui, action);
        self.turn();
    }

    /// The core's project state hash.
    pub fn state_hash(&self) -> u64 {
        EditorCore::read(&self.core, forge_cmd::Project::state_hash)
    }

    /// Whether the mirror equals the core's project.
    pub fn mirror_matches(&self) -> bool {
        EditorCore::read(&self.core, |p| self.shell.mirror().matches(p))
    }

    /// A panel's frame in the main window, if it is open there.
    pub fn panel_frame(&self, panel: &str) -> Option<forge_ui::WidgetId> {
        let dock = self.shell.dock().dock_of(forge_ui::dock::AreaId::Main)?;
        let f = forge_ui::dock::panel_frame_id(dock, &forge_ui::dock::PanelId::new(panel));
        self.h.ui.contains(f).then_some(f)
    }

    /// Show exactly `panels` in the main dock area, side by side (all visible).
    pub fn show_panels(&mut self, panels: &[&str]) -> Result<(), UiError> {
        use forge_ui::dock::{Axis, DockNode, Layout};
        let n = panels.len().max(1) as f32;
        let root = if panels.len() == 1 {
            DockNode::tabs(panels)
        } else {
            DockNode::split(
                Axis::Horizontal,
                panels
                    .iter()
                    .map(|p| (1.0 / n, DockNode::tabs(&[p])))
                    .collect(),
            )
        };
        self.shell.dock_mut().set_layout(Layout::new(root));
        self.settle();
        Ok(())
    }
}
