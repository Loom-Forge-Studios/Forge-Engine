//! `CommandEmitter` — the only way panel code changes the project (Ch.21 §21.18, I7).
//!
//! A panel gets a `CommandEmitter` in its [`crate::panels::PanelCx`] and clones it into its
//! handlers. It can emit one command (one transaction, one undo entry), several commands as
//! one transaction, or run a **gesture**: a slider or gizmo drag that sends at most one
//! command per frame into one transaction and becomes one undo entry; Esc cancels it. It
//! can also preview a command (`dry_run`). The issuer is stamped by the client the shell
//! connected: a panel cannot forge one.
//!
//! The emitter holds the shell's [`BusClient`] — local or remote, a panel cannot tell.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use forge_cmd::{Diff, EditorCommand, Rejection, TxnId};

use crate::client::{BusClient, Ticket};

type Client = Rc<RefCell<Box<dyn BusClient>>>;

struct GestureState {
    txn: TxnId,
    /// The frame the last command was sent in.
    sent_frame: Option<u64>,
    /// A command that arrived in a frame that already sent one (it goes next frame).
    pending: Option<Vec<EditorCommand>>,
    /// W2 fault switch for `test_gesture_single_undo`'s positive control: every frame's
    /// command becomes its own transaction (what a gesture without a txn does).
    fault_no_txn: crate::controls::Switch,
}

struct Shared {
    client: Client,
    /// The shell's frame counter (bumped once per loop turn that has input).
    frame: Cell<u64>,
    gestures: RefCell<Vec<Weak<RefCell<GestureState>>>>,
    emitted: Cell<u64>,
}

/// See the module docs. Cheap to clone (one `Rc`).
#[derive(Clone)]
pub struct CommandEmitter {
    shared: Rc<Shared>,
}

impl std::fmt::Debug for CommandEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandEmitter")
            .field("frame", &self.shared.frame.get())
            .field("emitted", &self.shared.emitted.get())
            .finish_non_exhaustive()
    }
}

impl CommandEmitter {
    /// An emitter over the shell's client.
    pub fn new(client: Box<dyn BusClient>) -> Self {
        Self::over(Rc::new(RefCell::new(client)))
    }

    pub(crate) fn over(client: Client) -> Self {
        Self {
            shared: Rc::new(Shared {
                client,
                frame: Cell::new(0),
                gestures: RefCell::new(Vec::new()),
                emitted: Cell::new(0),
            }),
        }
    }

    pub(crate) fn client(&self) -> &Client {
        &self.shared.client
    }

    fn send(&self, cmd: EditorCommand, txn: Option<TxnId>) -> Ticket {
        self.shared.emitted.set(self.shared.emitted.get() + 1);
        self.shared.client.borrow_mut().apply(cmd, txn)
    }

    /// Who this emitter's commands are from (the shell's human issuer: panels name it in
    /// the user-config audit records they write, e.g. a device pairing).
    pub fn issuer(&self) -> forge_cmd::Issuer {
        self.shared.client.borrow().issuer().clone()
    }

    /// One command: one transaction, one undo entry.
    pub fn emit(&self, cmd: EditorCommand) -> Ticket {
        self.send(cmd, None)
    }

    /// Several commands that are one user intent: one transaction labelled `label`, one
    /// undo entry. A single command whose own label already says it all (or an empty
    /// `label`) is sent as its own transaction; otherwise it is wrapped so the undo history
    /// reads the intent ("Hide 1 entity"), not the raw command.
    pub fn emit_all(&self, label: &str, cmds: Vec<EditorCommand>) -> Vec<Ticket> {
        match cmds.len() {
            0 => Vec::new(),
            1 if label.is_empty() || cmds[0].label() == label => {
                cmds.into_iter().map(|c| self.emit(c)).collect()
            }
            _ => {
                let txn = self.shared.client.borrow_mut().begin(label);
                let mut out: Vec<Ticket> =
                    cmds.into_iter().map(|c| self.send(c, Some(txn))).collect();
                out.push(self.shared.client.borrow_mut().commit(txn));
                out
            }
        }
    }

    /// Start a continuous gesture (slider drag, gizmo drag, brush stroke).
    pub fn gesture(&self, label: &str) -> Gesture {
        let txn = self.shared.client.borrow_mut().begin(label);
        self.gesture_with(txn, crate::controls::Switch::default())
    }

    /// W2 positive control only: a "gesture" that sends each frame as its own transaction.
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn gesture_without_txn_for_control(&self, label: &str) -> Gesture {
        self.gesture_or_fault(label, crate::controls::Switch { on: true })
    }

    /// [`CommandEmitter::gesture`], or (a tool's W2 fault switch on, test builds only) a
    /// "gesture" that sends each frame as its own transaction. A production build's switch
    /// is zero-sized and off. Any plugin's tool uses it for its own W2 control.
    pub fn gesture_or_fault(&self, label: &str, fault_no_txn: crate::controls::Switch) -> Gesture {
        let txn = self.shared.client.borrow_mut().begin(label);
        self.gesture_with(txn, fault_no_txn)
    }

    fn gesture_with(&self, txn: TxnId, fault_no_txn: crate::controls::Switch) -> Gesture {
        let state = Rc::new(RefCell::new(GestureState {
            txn,
            sent_frame: None,
            pending: None,
            fault_no_txn,
        }));
        let mut g = self.shared.gestures.borrow_mut();
        g.retain(|w| w.strong_count() > 0);
        g.push(Rc::downgrade(&state));
        Gesture {
            em: self.clone(),
            state,
            done: false,
        }
    }

    /// What `cmd` would do, without doing it.
    pub fn preview(&self, cmd: &EditorCommand) -> Result<Diff, Rejection> {
        self.shared.client.borrow().preview(cmd)
    }

    /// Undo the newest undoable transaction (Ctrl+Z). `None`: nothing to undo.
    pub fn undo(&self) -> Option<Ticket> {
        let t = self.shared.client.borrow().undo_target()?;
        Some(self.shared.client.borrow_mut().undo(t))
    }

    /// Redo the most recently undone transaction (Ctrl+Y). `None`: nothing to redo.
    pub fn redo(&self) -> Option<Ticket> {
        let t = self.shared.client.borrow().redo_target()?;
        Some(self.shared.client.borrow_mut().redo(t))
    }

    /// Undo one transaction (selective undo; the history panel's "undo to" sends one of
    /// these per transaction, newest first).
    pub fn undo_txn(&self, txn: TxnId) -> Ticket {
        self.shared.client.borrow_mut().undo(txn)
    }

    /// Redo one undone transaction.
    pub fn redo_txn(&self, txn: TxnId) -> Ticket {
        self.shared.client.borrow_mut().redo(txn)
    }

    /// Walk the history (see [`crate::mirror::ProjectMirror::plan_travel`]): one bus call
    /// per transaction, in order.
    pub fn travel(&self, plan: &[crate::mirror::Travel]) -> Vec<Ticket> {
        plan.iter()
            .map(|t| match t {
                crate::mirror::Travel::Undo(txn) => self.undo_txn(*txn),
                crate::mirror::Travel::Redo(txn) => self.redo_txn(*txn),
            })
            .collect()
    }

    /// Commands sent through this emitter so far (tests, diagnostics).
    pub fn emitted(&self) -> u64 {
        self.shared.emitted.get()
    }

    /// The shell calls this once per loop turn: a gesture command held back because its
    /// frame had already sent one goes out now. Public for a plugin's gesture tests (they
    /// drive an emitter without a shell).
    pub fn next_frame(&self) {
        self.shared.frame.set(self.shared.frame.get() + 1);
        let live: Vec<Rc<RefCell<GestureState>>> = {
            let mut g = self.shared.gestures.borrow_mut();
            g.retain(|w| w.strong_count() > 0);
            g.iter().filter_map(Weak::upgrade).collect()
        };
        for st in live {
            let pending = st.borrow_mut().pending.take();
            if let Some(cmd) = pending {
                self.send_frame(&st, cmd);
            }
        }
    }

    fn send_frame(&self, st: &Rc<RefCell<GestureState>>, cmds: Vec<EditorCommand>) {
        let (txn, fault) = {
            let mut s = st.borrow_mut();
            s.sent_frame = Some(self.shared.frame.get());
            (s.txn, s.fault_no_txn.on())
        };
        for cmd in cmds {
            self.send(cmd, if fault { None } else { Some(txn) });
        }
    }
}

/// An open gesture (see [`CommandEmitter::gesture`]). Dropping it without
/// [`Gesture::commit`] cancels it: nothing half-done stays behind.
pub struct Gesture {
    em: CommandEmitter,
    state: Rc<RefCell<GestureState>>,
    done: bool,
}

impl std::fmt::Debug for Gesture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gesture")
            .field("txn", &self.state.borrow().txn)
            .finish_non_exhaustive()
    }
}

impl Gesture {
    /// The gesture's transaction.
    pub fn txn(&self) -> TxnId {
        self.state.borrow().txn
    }

    /// This frame's command. At most one command per frame is sent: a second one in the
    /// same frame replaces the held one, which goes out next frame (or at commit).
    pub fn update(&mut self, cmd: EditorCommand) {
        self.update_all(vec![cmd]);
    }

    /// This frame's commands, as one step of the gesture (a multi-object drag: the same
    /// property on every selected entity). The same rule as [`Gesture::update`]: at most
    /// one step per frame, a newer step in the same frame replaces the held one.
    pub fn update_all(&mut self, cmds: Vec<EditorCommand>) {
        if cmds.is_empty() {
            return;
        }
        let frame = self.em.shared.frame.get();
        let sent_this_frame = self.state.borrow().sent_frame == Some(frame);
        if sent_this_frame {
            self.state.borrow_mut().pending = Some(cmds);
        } else {
            self.em.send_frame(&self.state, cmds);
        }
    }

    /// Whether the last step given is still held back (it goes out next frame, or at
    /// commit, unless a newer step replaces it). When it is not, every step given so far
    /// has been sent — a gesture that sends deltas starts its next one from there.
    pub fn is_holding(&self) -> bool {
        self.state.borrow().pending.is_some()
    }

    /// Release: send any held command, then commit — one undo entry.
    pub fn commit(mut self) -> Ticket {
        self.done = true;
        let pending = self.state.borrow_mut().pending.take();
        if let Some(cmd) = pending {
            self.em.send_frame(&self.state, cmd);
        }
        let txn = self.txn();
        self.em.shared.client.borrow_mut().commit(txn)
    }

    /// Esc: revert everything the gesture applied.
    pub fn cancel(mut self) -> Ticket {
        self.done = true;
        self.state.borrow_mut().pending = None;
        let txn = self.txn();
        self.em.shared.client.borrow_mut().cancel(txn)
    }
}

impl Drop for Gesture {
    fn drop(&mut self) {
        if !self.done {
            let txn = self.state.borrow().txn;
            if let Ok(mut c) = self.em.shared.client.try_borrow_mut() {
                c.cancel(txn);
            }
        }
    }
}
