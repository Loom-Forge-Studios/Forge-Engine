//! What a panel's code can reach, and how its widgets talk back (Ch.21 §21.18).
//!
//! A panel gets [`ShellHandles`]: the [`ProjectMirror`] **read-only**, the
//! [`CommandEmitter`] (the only way to change the project), and the [`SessionState`]
//! (selection, notifications, editor settings, keymap — not project state). There is no
//! `Bus`, no `World` and no `ProjectStore` anywhere in reach; the I7 guard checks that
//! through the import graph.
//!
//! A panel built with [`crate::panels::PanelCx::add_live`] gets a [`PanelBuilder`]: it adds
//! widgets, registers **handlers** for the actions its widgets raise ([`PanelBuilder::on`],
//! routed by the shell from the widget up its ancestors), and **sync steps**
//! ([`PanelBuilder::sync`]) the shell runs in that panel's window when the mirror or the
//! session changed (or when a panel asked for another turn to continue bounded work,
//! [`PanelSync::want_turn`]) — the step compares the revisions it cares about and updates its own
//! widgets, so an unchanged panel does no work.

use std::any::{Any, TypeId};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use forge_ui::widgets::Build;
use forge_ui::{Ui, UiError, WidgetId};

use crate::core::EditorCore;
use crate::emitter::CommandEmitter;
use crate::mirror::ProjectMirror;
use crate::services::EditorServices;
use crate::session::SessionState;

/// The window a panel lives in (`None`: the main window; `Some(key)`: a floating window).
pub type WindowKey = Option<u64>;

/// What a handler gets when its action arrives.
pub struct PanelAct<'a> {
    pub ui: &'a mut Ui,
    /// The widget that raised the action.
    pub source: WidgetId,
    pub mirror: &'a ProjectMirror,
    pub cmd: &'a CommandEmitter,
    pub session: &'a mut SessionState,
    pub services: &'a EditorServices,
    turn: &'a Cell<bool>,
}

impl PanelAct<'_> {
    /// Ask for another loop turn in which this window's sync steps run even if nothing
    /// changed: a panel doing bounded work in slices (a filter over 100,000 rows) keeps
    /// asking until it is done. Idle panels never ask, so the idle loop still sleeps.
    pub fn want_turn(&self) {
        self.turn.set(true);
    }
}

/// What a sync step gets.
pub struct PanelSync<'a> {
    pub ui: &'a mut Ui,
    pub mirror: &'a ProjectMirror,
    pub session: &'a SessionState,
    pub cmd: &'a CommandEmitter,
    pub services: &'a EditorServices,
    turn: &'a Cell<bool>,
}

impl PanelSync<'_> {
    /// See [`PanelAct::want_turn`].
    pub fn want_turn(&self) {
        self.turn.set(true);
    }
}

type Handler = Box<dyn FnMut(&mut PanelAct, &dyn Any)>;
type SyncFn = Box<dyn FnMut(&mut PanelSync) -> Result<(), UiError>>;
type OpFn = Box<dyn FnMut(&mut PanelAct)>;

struct SyncEntry {
    /// The widget whose existence keeps the step alive (the panel's frame content).
    owner: WidgetId,
    f: SyncFn,
}

/// Handlers and sync steps of the panels built in one window.
#[derive(Default)]
struct WindowWiring {
    handlers: HashMap<(WidgetId, TypeId), Handler>,
    syncs: Vec<SyncEntry>,
    ops: BTreeMap<String, (WidgetId, OpFn)>,
    /// A panel asked for another loop turn (bounded work still to do).
    turn_wanted: bool,
}

/// Every window's wiring (the shell owns it; panels add to it through the builder).
#[derive(Default)]
pub struct Wiring {
    windows: HashMap<WindowKey, WindowWiring>,
}

impl std::fmt::Debug for Wiring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wiring")
            .field("windows", &self.windows.len())
            .finish_non_exhaustive()
    }
}

impl Wiring {
    /// Handlers registered in `window` (tests, diagnostics).
    pub fn handler_count(&self, window: WindowKey) -> usize {
        self.windows.get(&window).map_or(0, |w| w.handlers.len())
    }
    pub fn sync_count(&self, window: WindowKey) -> usize {
        self.windows.get(&window).map_or(0, |w| w.syncs.len())
    }
    /// Forget a closed window.
    pub fn drop_window(&mut self, window: WindowKey) {
        self.windows.remove(&window);
    }
}

/// The handles a panel gets (see the module docs). Cheap to clone.
#[derive(Clone)]
pub struct ShellHandles {
    mirror: Rc<RefCell<ProjectMirror>>,
    cmd: CommandEmitter,
    session: Rc<RefCell<SessionState>>,
    pub(crate) wiring: Rc<RefCell<Wiring>>,
    services: Rc<EditorServices>,
}

impl std::fmt::Debug for ShellHandles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellHandles").finish_non_exhaustive()
    }
}

impl ShellHandles {
    pub(crate) fn new(
        mirror: Rc<RefCell<ProjectMirror>>,
        cmd: CommandEmitter,
        session: Rc<RefCell<SessionState>>,
        wiring: Rc<RefCell<Wiring>>,
        services: Rc<EditorServices>,
    ) -> Self {
        Self {
            mirror,
            cmd,
            session,
            wiring,
            services,
        }
    }

    /// The read-side services (components, inspector widgets, console log, assets, and
    /// the services plugins add).
    pub fn services(&self) -> &EditorServices {
        &self.services
    }
    pub fn services_rc(&self) -> Rc<EditorServices> {
        Rc::clone(&self.services)
    }

    /// A builder over these handles, for a panel that rebuilds part of itself later (the
    /// inspector on a selection change): it wires handlers and sync steps exactly as the
    /// panel's first build did.
    pub fn builder<'b>(
        &'b self,
        b: &'b mut dyn Build,
        parent: WidgetId,
        window: WindowKey,
    ) -> PanelBuilder<'b> {
        PanelBuilder::new(b, parent, self, window)
    }

    /// Handles over a fresh, empty in-memory core with a test issuer: for building a panel
    /// outside a running shell (the preset guard, empty-state audits). Nothing it emits
    /// reaches any real project.
    pub fn detached() -> Self {
        let core = EditorCore::new();
        let client = EditorCore::connect(&core, forge_cmd::Issuer::Test);
        Self::new(
            Rc::new(RefCell::new(ProjectMirror::new())),
            CommandEmitter::new(Box::new(client)),
            Rc::new(RefCell::new(SessionState::detached())),
            Rc::new(RefCell::new(Wiring::default())),
            Rc::new(EditorServices::default()),
        )
    }

    /// The project, read-only.
    pub fn mirror(&self) -> Ref<'_, ProjectMirror> {
        self.mirror.borrow()
    }
    /// The only way to change the project.
    pub fn cmd(&self) -> &CommandEmitter {
        &self.cmd
    }
    /// Session state and user config.
    pub fn session(&self) -> Ref<'_, SessionState> {
        self.session.borrow()
    }
    pub fn session_mut(&self) -> RefMut<'_, SessionState> {
        self.session.borrow_mut()
    }
    pub(crate) fn mirror_rc(&self) -> &Rc<RefCell<ProjectMirror>> {
        &self.mirror
    }

    /// W2 positive controls only: break a guarded property of the mirror (see
    /// [`crate::mirror::MirrorFaults`]).
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn set_mirror_faults(&self, f: crate::mirror::MirrorFaults) {
        self.mirror.borrow_mut().faults = f;
    }

    /// Route one action raised in `window` by `source`: the nearest handler for its type on
    /// the source or an ancestor runs. Returns whether one ran.
    pub(crate) fn dispatch(
        &self,
        ui: &mut Ui,
        window: WindowKey,
        source: WidgetId,
        action: &dyn Any,
    ) -> bool {
        let ty = action.type_id();
        let mut cur = Some(source);
        while let Some(id) = cur {
            let taken = self
                .wiring
                .borrow_mut()
                .windows
                .get_mut(&window)
                .and_then(|w| w.handlers.remove(&(id, ty)));
            if let Some(mut h) = taken {
                let turn = Cell::new(false);
                {
                    let mirror = self.mirror.borrow();
                    let mut session = self.session.borrow_mut();
                    let mut act = PanelAct {
                        ui,
                        source,
                        mirror: &mirror,
                        cmd: &self.cmd,
                        session: &mut session,
                        services: &self.services,
                        turn: &turn,
                    };
                    h(&mut act, action);
                }
                if turn.get() {
                    self.want_turn(window);
                }
                // Put it back unless the handler's widget went away meanwhile.
                if ui.contains(id) {
                    self.wiring
                        .borrow_mut()
                        .windows
                        .entry(window)
                        .or_default()
                        .handlers
                        .entry((id, ty))
                        .or_insert(h);
                }
                return true;
            }
            cur = ui.parent(id);
        }
        false
    }

    /// Run a named session operation (a plugin's `SessionOp::Custom`). Returns whether a
    /// live panel in `window` handles it.
    pub(crate) fn run_op(&self, ui: &mut Ui, window: WindowKey, name: &str) -> bool {
        let taken = self
            .wiring
            .borrow_mut()
            .windows
            .get_mut(&window)
            .and_then(|w| w.ops.remove(name));
        let Some((owner, mut f)) = taken else {
            return false;
        };
        if !ui.contains(owner) {
            return false;
        }
        let turn = Cell::new(false);
        {
            let mirror = self.mirror.borrow();
            let mut session = self.session.borrow_mut();
            let mut act = PanelAct {
                ui,
                source: owner,
                mirror: &mirror,
                cmd: &self.cmd,
                session: &mut session,
                services: &self.services,
                turn: &turn,
            };
            f(&mut act);
        }
        if turn.get() {
            self.want_turn(window);
        }
        self.wiring
            .borrow_mut()
            .windows
            .entry(window)
            .or_default()
            .ops
            .entry(name.to_string())
            .or_insert((owner, f));
        true
    }

    /// Run `window`'s sync steps; steps whose panel is gone are dropped (and so are their
    /// handlers).
    pub(crate) fn sync(&self, ui: &mut Ui, window: WindowKey) -> Result<(), UiError> {
        let mut syncs = match self.wiring.borrow_mut().windows.get_mut(&window) {
            Some(w) => {
                w.handlers.retain(|(id, _), _| ui.contains(*id));
                w.ops.retain(|_, (id, _)| ui.contains(*id));
                std::mem::take(&mut w.syncs)
            }
            None => return Ok(()),
        };
        syncs.retain(|s| ui.contains(s.owner));
        let mut result = Ok(());
        let turn = Cell::new(false);
        {
            let mirror = self.mirror.borrow();
            let session = self.session.borrow();
            let mut cx = PanelSync {
                ui,
                mirror: &mirror,
                session: &session,
                cmd: &self.cmd,
                services: &self.services,
                turn: &turn,
            };
            for s in &mut syncs {
                if let Err(e) = (s.f)(&mut cx) {
                    result = Err(e);
                }
            }
        }
        let mut w = self.wiring.borrow_mut();
        let ww = w.windows.entry(window).or_default();
        // Steps added while syncing (a panel rebuilt itself) stay after the old ones.
        syncs.append(&mut ww.syncs);
        ww.syncs = syncs;
        ww.turn_wanted |= turn.get();
        result
    }

    fn want_turn(&self, window: WindowKey) {
        self.wiring
            .borrow_mut()
            .windows
            .entry(window)
            .or_default()
            .turn_wanted = true;
    }

    /// Did a panel in `window` ask for another turn? Clears the request.
    pub(crate) fn take_turn_wanted(&self, window: WindowKey) -> bool {
        self.wiring
            .borrow_mut()
            .windows
            .get_mut(&window)
            .is_some_and(|w| std::mem::take(&mut w.turn_wanted))
    }

    /// Does a panel in any window want another turn?
    pub(crate) fn turn_wanted(&self) -> bool {
        self.wiring.borrow().windows.values().any(|w| w.turn_wanted)
    }
}

/// What [`crate::panels::PanelCx::add_live`] steps build with (see the module docs).
pub struct PanelBuilder<'a> {
    pub b: &'a mut dyn Build,
    /// The panel's frame: add widgets under it.
    pub parent: WidgetId,
    shell: &'a ShellHandles,
    window: WindowKey,
}

impl<'a> PanelBuilder<'a> {
    pub(crate) fn new(
        b: &'a mut dyn Build,
        parent: WidgetId,
        shell: &'a ShellHandles,
        window: WindowKey,
    ) -> Self {
        Self {
            b,
            parent,
            shell,
            window,
        }
    }

    /// The project, read-only.
    pub fn mirror(&self) -> Ref<'_, ProjectMirror> {
        self.shell.mirror()
    }
    /// The command emitter (clone it into handlers that need it later).
    pub fn cmd(&self) -> CommandEmitter {
        self.shell.cmd().clone()
    }
    pub fn session(&self) -> Ref<'_, SessionState> {
        self.shell.session()
    }
    pub fn session_mut(&self) -> RefMut<'_, SessionState> {
        self.shell.session_mut()
    }
    /// The window being built in.
    pub fn window(&self) -> WindowKey {
        self.window
    }
    /// The handles themselves, for closures that outlive the build (signal effects).
    pub fn handles(&self) -> ShellHandles {
        self.shell.clone()
    }
    /// The read-side services (see [`crate::services`]).
    pub fn services(&self) -> Rc<EditorServices> {
        self.shell.services_rc()
    }

    /// Run this window's sync steps on the next loop turn even if nothing changed: a
    /// panel that fills its rows from the mirror in its sync step shows them at once.
    pub fn want_turn(&self) {
        self.shell.want_turn(self.window);
    }

    /// Run `f` when widget `id` (or a descendant without its own handler) raises an `A`.
    pub fn on<A: Any>(&mut self, id: WidgetId, mut f: impl FnMut(&mut PanelAct, &A) + 'static) {
        let h: Handler = Box::new(move |cx, a| {
            if let Some(a) = a.downcast_ref::<A>() {
                f(cx, a);
            }
        });
        self.shell
            .wiring
            .borrow_mut()
            .windows
            .entry(self.window)
            .or_default()
            .handlers
            .insert((id, TypeId::of::<A>()), h);
    }

    /// Run `f` in this window whenever the mirror or the session changed, while `owner`
    /// exists. `f` checks the revisions it cares about.
    pub fn sync(
        &mut self,
        owner: WidgetId,
        f: impl FnMut(&mut PanelSync) -> Result<(), UiError> + 'static,
    ) {
        self.shell
            .wiring
            .borrow_mut()
            .windows
            .entry(self.window)
            .or_default()
            .syncs
            .push(SyncEntry {
                owner,
                f: Box::new(f),
            });
    }

    /// Handle the named session operation (an action of kind `SessionOp::Custom(name)`)
    /// while `owner` exists: F2 → rename in the hierarchy, Ctrl+L → clear the console.
    pub fn on_op(&mut self, owner: WidgetId, name: &str, f: impl FnMut(&mut PanelAct) + 'static) {
        self.shell
            .wiring
            .borrow_mut()
            .windows
            .entry(self.window)
            .or_default()
            .ops
            .insert(name.to_string(), (owner, Box::new(f)));
    }
}
