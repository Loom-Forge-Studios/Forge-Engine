//! Session state and user config as the shell holds them (Ch.21 §21.18: the two classes
//! of state that are **not** on the bus).
//!
//! * Session state: selection, the focused panel, the notification centre, requests a
//!   panel makes of the shell (open a panel, run an action).
//! * User config: editor settings and the keymap. They are written to the per-user config
//!   directory by the shell's debounced, crash-safe saver — never to the project.
//!
//! Panels may change these directly: they are not project state. Everything a panel wants
//! to change in the project goes through the `CommandEmitter` instead.

use std::cell::RefCell;
use std::rc::Rc;

use forge_cmd::EntityKey;
use forge_ui::dock::PanelId;

use crate::EditorError;
use crate::keybind::ActionSummary;
use crate::keymap::{Chord, KeyContext, KeyMap, KeymapFile};
use crate::notify::{Level, NoticeAction, NotificationCentre};
use crate::settings::EditorSettings;

/// Something a panel asks the shell to do (session state only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellRequest {
    OpenPanel(PanelId),
    RunAction(String),
}

/// Where the editor camera should go: a navigator panel's "Fly to" an addressed node (Ch.21
/// §21.21). A **session** camera action — never a command, never project state; the
/// viewport (WP-U6) follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraFocus {
    /// The addressed node (`universe/world:3/zone:118/tree:2`).
    pub seed_path: String,
    /// Its display name.
    pub name: String,
}

/// Something a panel should reveal (a console entry's click-through, a seed path clicked
/// in the inspector): the panel that owns that kind of thing selects and scrolls to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reveal {
    /// Generated content's seed path (the panel a [`SeedPathPanel`] names).
    SeedPath(String),
    /// A transaction (the undo history).
    Txn(forge_cmd::TxnId),
    /// A node of a graph (the graph editor, WP-U8).
    GraphNode { graph: String, node: u64 },
    /// A console entry.
    LogEntry(u64),
}

/// The panel that reveals a seed path ([`Reveal::SeedPath`]), in the services' typed
/// extension slot ([`ServiceExtensions`](crate::services::ServiceExtensions)): a plugin whose
/// panel browses generated content by seed path adds one from its service provider. Without
/// one no panel can show a seed path, so the inspector shows generated content's seed path
/// with no reveal button and a console entry's seed path is not a click-through — the
/// editor never offers an action that opens nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedPathPanel {
    /// The panel [`Reveal::SeedPath`] opens.
    pub panel: PanelId,
    /// Its display name, as the reveal button reads it ("Show in {title}").
    pub title: String,
}

impl SeedPathPanel {
    /// The seed-path panel a plugin added to `services`, if any.
    #[must_use]
    pub fn of(services: &crate::services::EditorServices) -> Option<Rc<SeedPathPanel>> {
        services.extensions.get::<SeedPathPanel>()
    }

    /// Reveal `path` in this panel (opening it).
    pub fn reveal(&self, session: &mut SessionState, path: String) {
        session.reveal(Reveal::SeedPath(path), self.panel.as_str());
    }
}

/// See the module docs.
#[derive(Debug)]
pub struct SessionState {
    pub selection: Vec<EntityKey>,
    pub focused_panel: Option<PanelId>,
    pub notifications: NotificationCentre,
    settings: EditorSettings,
    settings_rev: u64,
    keymap: Rc<RefCell<KeyMap>>,
    /// The keymap layers under the user's (defaults, then the preset's): what "reset"
    /// returns a binding to.
    base_keymap: KeyMap,
    keymap_rev: u64,
    requests: Vec<ShellRequest>,
    revision: u64,
    actions: Vec<ActionSummary>,
    camera: Option<CameraFocus>,
    camera_rev: u64,
    reveal: Option<(u64, Reveal)>,
    reveal_seq: u64,
    /// The scene the viewport isolates ("Open base scene", WP-U20); `None`: the world.
    scene_focus: Option<EntityKey>,
    scene_focus_rev: u64,
    selection_rev: u64,
    /// Whether this editor has a per-user config directory (dismissed first-run tips can
    /// be remembered only there, so tips show only then).
    pub user_config: bool,
}

impl SessionState {
    pub fn new(settings: EditorSettings, keymap: Rc<RefCell<KeyMap>>, base_keymap: KeyMap) -> Self {
        Self {
            selection: Vec::new(),
            focused_panel: None,
            notifications: NotificationCentre::new(),
            settings,
            settings_rev: 0,
            keymap,
            base_keymap,
            keymap_rev: 0,
            requests: Vec::new(),
            revision: 0,
            actions: Vec::new(),
            camera: None,
            camera_rev: 0,
            reveal: None,
            reveal_seq: 0,
            scene_focus: None,
            scene_focus_rev: 0,
            selection_rev: 0,
            user_config: false,
        }
    }

    // ---- camera focus and reveal (session state) ----------------------------------------

    /// Fly the editor camera to an addressed node (a session action; see [`CameraFocus`]).
    pub fn fly_to(&mut self, focus: CameraFocus) {
        self.camera = Some(focus);
        self.camera_rev += 1;
        self.touch();
    }
    /// Open a scene for editing in isolation (the viewport shows only it; `None`: back to
    /// the world). Session state: what is shown, never project state (WP-U20).
    pub fn open_scene(&mut self, root: Option<EntityKey>) {
        if self.scene_focus != root {
            self.scene_focus = root;
            self.scene_focus_rev += 1;
            self.touch();
        }
    }
    /// The scene open in isolation, if any.
    pub fn scene_focus(&self) -> Option<EntityKey> {
        self.scene_focus
    }
    /// Bumped whenever [`SessionState::open_scene`] changes it.
    pub fn scene_focus_revision(&self) -> u64 {
        self.scene_focus_rev
    }
    pub fn camera_focus(&self) -> Option<&CameraFocus> {
        self.camera.as_ref()
    }
    pub fn camera_revision(&self) -> u64 {
        self.camera_rev
    }
    /// Ask the panel owning `r`'s kind to reveal it; opens `panel` too.
    pub fn reveal(&mut self, r: Reveal, panel: &str) {
        self.reveal_seq += 1;
        self.reveal = Some((self.reveal_seq, r));
        self.request(ShellRequest::OpenPanel(PanelId::new(panel)));
    }
    /// The newest reveal request and its sequence number (panels remember the last one
    /// they acted on).
    pub fn pending_reveal(&self) -> Option<&(u64, Reveal)> {
        self.reveal.as_ref()
    }
    /// Bumped whenever the selection changes (the inspector's rebuild check).
    pub fn selection_revision(&self) -> u64 {
        self.selection_rev
    }

    /// Every registered action (the keybindings editor lists them all).
    pub fn actions(&self) -> &[ActionSummary] {
        &self.actions
    }
    pub(crate) fn set_actions(&mut self, actions: Vec<ActionSummary>) {
        self.actions = actions;
        self.touch();
    }
    /// The keymap without the user's changes (defaults + preset).
    pub fn base_keymap(&self) -> &KeyMap {
        &self.base_keymap
    }

    /// A session with defaults (panel tests, a detached panel context).
    pub fn detached() -> Self {
        let km = crate::keymap::default_keymap()
            .map(|f| KeyMap::from_layers(&f, &[]).0)
            .unwrap_or_default();
        Self::new(
            EditorSettings::default(),
            Rc::new(RefCell::new(km.clone())),
            km,
        )
    }

    /// Bumped by every change here (the shell's "anything to sync?" check, with the
    /// mirror's revision).
    pub fn revision(&self) -> u64 {
        self.revision + self.notifications.revision()
    }

    fn touch(&mut self) {
        self.revision += 1;
    }

    // ---- editor settings (user config) ---------------------------------------------------

    pub fn settings(&self) -> &EditorSettings {
        &self.settings
    }
    pub fn settings_revision(&self) -> u64 {
        self.settings_rev
    }
    /// Change the editor settings. The shell applies them to every window and saves them.
    pub fn update_settings(&mut self, f: impl FnOnce(&mut EditorSettings)) {
        let before = self.settings.clone();
        f(&mut self.settings);
        if self.settings != before {
            self.settings_rev += 1;
            self.touch();
        }
    }

    // ---- keymap (user config) ------------------------------------------------------------

    pub fn keymap(&self) -> &Rc<RefCell<KeyMap>> {
        &self.keymap
    }
    pub fn keymap_revision(&self) -> u64 {
        self.keymap_rev
    }
    /// Give `action` the chord in `context`. A conflict changes nothing and names both
    /// actions; the keybindings editor shows it.
    pub fn rebind(
        &mut self,
        action: &str,
        context: KeyContext,
        chord: Chord,
    ) -> Result<(), EditorError> {
        self.keymap.borrow_mut().rebind(action, context, chord)?;
        self.keymap_rev += 1;
        self.touch();
        Ok(())
    }
    /// Put `action`'s bindings back to the defaults (and the preset's). A default chord
    /// that the user has since given to another action is reported, not stolen.
    pub fn reset_binding(&mut self, action: &str) -> Result<(), EditorError> {
        let defaults: Vec<_> = self
            .base_keymap
            .bindings()
            .iter()
            .filter(|b| b.action == action)
            .cloned()
            .collect();
        let before = self.keymap.borrow().clone();
        let mut km = self.keymap.borrow_mut();
        let ours: Vec<_> = km
            .bindings()
            .iter()
            .filter(|b| b.action == action)
            .map(|b| (b.chord.clone(), b.context.clone()))
            .collect();
        for (c, ctx) in ours {
            km.unbind(&c, &ctx);
        }
        for b in defaults {
            let r = match &b.shadows {
                Some(s) => km.bind_shadowing(b.chord.clone(), b.context.clone(), &b.action, s),
                None => km.bind(b.chord.clone(), b.context.clone(), &b.action),
            };
            if let Err(e) = r {
                *km = before;
                return Err(e);
            }
        }
        drop(km);
        self.keymap_rev += 1;
        self.touch();
        Ok(())
    }
    /// Every binding back to the defaults.
    pub fn reset_all_bindings(&mut self) {
        *self.keymap.borrow_mut() = self.base_keymap.clone();
        self.keymap_rev += 1;
        self.touch();
    }
    /// The user's keymap layer: what differs from the defaults (what is saved).
    pub fn user_keymap_layer(&self) -> KeymapFile {
        let km = self.keymap.borrow();
        let base = self.base_keymap.bindings();
        let bindings = km
            .bindings()
            .iter()
            .filter(|b| !base.contains(b))
            .cloned()
            .collect();
        let unbind = base
            .iter()
            .filter(|b| {
                !km.bindings()
                    .iter()
                    .any(|x| x.chord == b.chord && x.context == b.context)
            })
            .map(|b| crate::keymap::Unbind {
                chord: b.chord.clone(),
                context: b.context.clone(),
            })
            .collect();
        KeymapFile {
            version: 1,
            bindings,
            unbind,
        }
    }

    // ---- requests and notices ------------------------------------------------------------

    pub fn request(&mut self, r: ShellRequest) {
        self.requests.push(r);
        self.touch();
    }
    pub(crate) fn take_requests(&mut self) -> Vec<ShellRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Post information or a success (toast + history). A warning or an error is a
    /// [`Problem`](crate::notify::Problem): [`SessionState::problem`].
    pub fn notify(&mut self, level: Level, title: &str, detail: &str) -> u64 {
        self.notifications.post(level, title, detail, Vec::new())
    }

    /// Post information or a success with action buttons.
    pub fn notify_with(
        &mut self,
        level: Level,
        title: &str,
        detail: &str,
        actions: Vec<NoticeAction>,
    ) -> u64 {
        self.notifications.post(level, title, detail, actions)
    }

    /// Post a warning or an error: its code, what happened, why, and what to do next (M2-70).
    pub fn problem(&mut self, p: crate::notify::Problem) -> u64 {
        self.notifications.post_problem(p)
    }

    /// A panel refused an input before sending a command (`EDITOR-0014`): a warning naming
    /// what and why, and the next step.
    pub fn refuse(&mut self, what: &str, why: &str) -> u64 {
        let e = crate::EditorError::Refused {
            what: what.to_string(),
            why: why.to_string(),
        };
        self.problem(crate::notify::Problem::warning(
            e.code(),
            what,
            why,
            e.next_step(),
        ))
    }

    pub fn set_selection(&mut self, keys: Vec<EntityKey>) {
        if self.selection != keys {
            self.selection = keys;
            self.selection_rev += 1;
            self.touch();
        }
    }
}
