//! The editor's **headless core** (Ch.7.1, Ch.34 §34.4): the command bus that owns the
//! project, and nothing else. It has no UI, no window and no `forge-ui` dependency — the
//! same core runs under the GUI shell, under `forge-editor --headless`, and (M2-16) behind
//! the split editor's remote transport.
//!
//! Clients reach it only through [`LocalBus`], the in-process [`BusClient`]. Every client
//! has its own issuer and its own `Applied` subscription, so a script's or an automation session's
//! change reaches the UI by exactly the path the UI's own changes take (Ch.21 §21.18). A
//! change made by one client wakes the others (their loop sleeps otherwise, D-5).
//!
//! This is the only module of the editor that names [`forge_cmd::Bus`]; the I7 guard
//! (`tests/liveness/test_command_liveness.rs`) refuses a reference to it from anywhere
//! else in the editor or its panel plugins.
//!
//! **The one grant table (E-27, WP-U9).** The core owns the editor's [`SharedGrants`]: the
//! table the WASM plugin host and every session host check at every call. It follows the
//! project: after every change the core applies, it runs [`GrantSync`] over the applied
//! diffs (`security.*` settings, which the core reserves on its bus to the human-only security
//! commands and its own project load), and
//! writes what changed to its [`AuditBook`]. It also performs a human's approval or
//! rejection of an automation session's preview transaction (`forge.automation.approve` / `reject`)
//! once the bus accepted the command.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use forge_cmd::{
    Bus, CommandSink, Diff, EditorCommand, Issuer, Project, Rejection, Subscription, TxnId,
    TxnState, Value,
};
use forge_plugin::SharedGrants;
use forge_project::ProjectError;
use forge_project::format::ProjectDoc;
use forge_project::host::{Credential, DeviceCode, GitHub, Keep, ProjectHost, StoreOpener};
use forge_project::status::ProjectStatus;

use crate::client::{BusClient, Pumped, Refused, SessionCommand, Ticket, TxnInfo};
use crate::connect::audit::{AuditBook, AuditOrigin, AuditRecord};
use crate::project::{self as lifecycle, ProjectOp};
use crate::security::{self, GrantSync, SecurityEvent};

// WP-U10: the core's side of teams and sandboxes (Ch.37): collaboration commands checked,
// performed and followed under the core's lock.
mod collab_host;
pub use collab_host::{CollabAttach, CollabCounters};

/// Wakes a client's event loop from another thread.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The headless core (see the module docs).
pub struct EditorCore {
    bus: Bus,
    wakers: BTreeMap<u64, Waker>,
    next_client: u64,
    /// `Invoke` targets that are session commands (see [`SessionCommand`]).
    session_targets: BTreeSet<String>,
    /// The session-command inboxes of the clients that follow them.
    inboxes: BTreeMap<u64, Inbox>,
    /// The project's store host (E-36: the core performs the store operations the
    /// lifecycle commands request; only the core names it, I7).
    project: ProjectHost,
    /// The presets the open project works with (its own copies in place of the built-in
    /// ones, WP-16): the promote command's planner reads them.
    presets: lifecycle::promote::PresetDefaults,
    /// The promotion rules the plugins add (the `PromotionRule` registry, attached with the
    /// plugin commands): the promote command's planner reads them.
    rules: lifecycle::promote::SharedRules,
    /// The threads of the lifecycle transfers started (push, pull, clone, sign-in,
    /// repository creation: their network part runs off the core's lock, WP-16).
    transfers: Vec<std::thread::JoinHandle<()>>,
    faults: CoreFaults,
    /// The one grant table (see the module docs).
    grants: SharedGrants,
    /// Keeps `grants` equal to what the project grants.
    sync: GrantSync,
    /// The core's own view of the `Applied` stream, for `sync` and the audit book.
    watch: Subscription,
    /// Security commands and approvals, as they apply (the audit panel reads it).
    audit: AuditBook,
    /// Teams and sandboxes (WP-U10): `None` until backends are attached.
    collab: Option<Box<collab_host::CollabHost>>,
    /// Told when the collaboration status changes (the panels' feeds).
    collab_watchers: Vec<forge_project::collab::Observer>,
    collab_gen: u64,
    /// The clients' wakers, reachable without the core's lock: a collaboration backend
    /// wakes every client when a teammate publishes (it may do so while another core's lock
    /// is held, never this one's).
    wake_list: Arc<Mutex<BTreeMap<u64, Waker>>>,
    /// The next automation session number: one count for every session host over this core, so
    /// `auto-N` names one session only (WP-20: a session host numbers its sessions from here).
    next_automation_session: u64,
    /// Security settings a non-human's load held back for a person (WP-33): shown in the
    /// Plugin manager until a human accepts or discards them, or another load replaces
    /// the project.
    held: Option<security::HeldSecurity>,
    next_held: u64,
    /// Where the person's project-trust answers are remembered (WP-34, [`crate::trust`]):
    /// the GUI's is the user config file; a bare core remembers for this run only.
    trust_book: Arc<dyn crate::trust::TrustBook>,
    /// What the open project carries that needs trust, and where it stands (`None`: it
    /// carries nothing to run or grant).
    trust: Option<crate::trust::ProjectTrust>,
    next_trust: u64,
    /// Moves whenever an answer the trust gate reads could change (WP-36): the trust book or
    /// an answer in it, the open project (a load), the core's faults. The hosting caches its
    /// vets against it, so an idle poll takes no core lock per plugin.
    trust_version: Arc<std::sync::atomic::AtomicU64>,
    /// The `seq` of the loads and team pulls the core applied itself (WP-35): a person's
    /// command that changes a plugin grant, the automation policy or the plugin set moves their
    /// trust answer with it ([`EditorCore::follow_own_trust`]); a load or a pull brings what
    /// the files or the team carry, which the person has not seen.
    core_applied: std::collections::BTreeSet<u64>,
    /// The presets a new project is created from: the editor's real `Preset` registry
    /// (built-in and plugin presets, WP-21), shared with the shell that hosts the plugins.
    /// A bare core has the built-in three.
    catalog: crate::presets::SharedPresets,
}

forge_trace::control_switches! {
    /// W2 positive-control switches for the core's guards. Never set outside those tests.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct CoreFaults {
        /// A push holds the core's lock across its replay, its upload (`publish`) and recording
        /// the new head: the network under the lock, as before transfers ran off it (the
        /// control of `every_transfer_runs_off_the_core_lock_…` in `test_git_remote_history`).
        pub push_publishes_under_lock: bool,
        /// A client's batch is previewed on a copy of the whole project and then planned a
        /// second time to apply, as before WP-20 (the control of `test_batch_apply_cost`: the
        /// cost of a small batch then grows with the project).
        pub batch_copies_project: bool,
        /// A load a non-human started takes effect like a human's: the plugin grants and the
        /// default automation policy its file carries apply at once, as before WP-33
        /// (`test_automation_open_security`'s control: an automation session then widens them by
        /// opening a file).
        pub automation_load_unheld: bool,
        /// A save writes the per-run settings (automation grants and this run's epoch) into the
        /// project's files, as before WP-33 (`test_automation_open_security`'s save control).
        pub save_keeps_per_run: bool,
        /// A save or a build while security settings are held writes the project as the load
        /// left it in memory, as before the WP-33 fix: the file's plugin grants and default
        /// automation policy are reverted on disk (`test_automation_open_security`'s
        /// held-save control).
        pub save_writes_held: bool,
        /// A non-human's load or team pull that restricts plugin grants or the default automation
        /// session policy is not audited as `narrowed`, as before WP-21
        /// (`test_automation_open_security`'s narrowed control: a dropped grant then leaves no
        /// trace a person can find).
        pub narrowed_unaudited: bool,
        /// A load of a project nobody trusted takes effect like a trusted one's: the plugin
        /// grants its files carry apply at once, as before WP-34 (`test_project_trust`'s grant
        /// control: a cloned project then grants its own plugin code).
        pub untrusted_load_unheld: bool,
        /// The core lets a client send the core-only `forge.collab.sync` like any other command
        /// (`test_collab_sync_not_sendable`'s control: an automation session then writes a plugin
        /// grant through it, the sync being a reserved writer of `security.*`).
        pub core_only_sendable: bool,
        /// Trust is keyed by the project folder only, as before WP-35: a trusted folder's
        /// new or changed plugins and new grants take effect without asking
        /// (`test_project_trust`'s pull control: a pulled plugin then runs with its pulled
        /// grant).
        pub trust_by_folder: bool,
    }
}

/// Session commands a client may hold undrained; beyond it the oldest go and are counted.
pub const SESSION_INBOX: usize = 4096;

#[derive(Debug, Default)]
struct Inbox {
    items: VecDeque<SessionCommand>,
    dropped: u64,
}

/// The core, shared by its clients (an automation session runs on another thread).
pub type SharedCore = Arc<Mutex<EditorCore>>;

fn lock(core: &SharedCore) -> MutexGuard<'_, EditorCore> {
    // The bus catches command panics at its own boundary (CMD-0009), so a poisoned lock
    // can only come from a panic outside a command; its state is still consistent.
    core.lock().unwrap_or_else(PoisonError::into_inner)
}

impl EditorCore {
    /// A core over an empty project, with the editor's first-party command handlers (the
    /// asset commands `forge.asset.*`, Ch.8.6) registered.
    pub fn new() -> SharedCore {
        Self::with_bus(Self::editor_bus())
    }

    /// A fresh bus with the editor's first-party handlers.
    pub fn editor_bus() -> Bus {
        let mut bus = Bus::new();
        Self::register_editor_handlers(&mut bus);
        bus
    }

    /// Register the editor's first-party handlers on `bus`. A play or lifecycle target `bus`
    /// already has keeps its handler (the I15 guard puts a preset-gated variant of a real
    /// command first, to prove it catches one).
    pub fn register_editor_handlers(bus: &mut Bus) {
        // Registering on a fresh bus cannot collide; if it ever did, the asset commands would
        // be refused as unknown (CMD-0010) and the refusal shown — never a silent failure.
        let _ = forge_asset::commands::register_commands(bus);
        // The play controls (Ch.21 §21.21, M2-33): a session command on the core, so an
        // automation session, a script or a remote client plays, pauses, steps and stops like the
        // panel.
        let (target, policy, plan) = crate::play::handler();
        let _ = bus.register_handler(target, policy, plan);
        // Simulation inputs (WP-19): a session command too, so a game's input in the editor,
        // a script, an automation session and a headless run feed the play core by one route.
        let (target, policy, plan) = crate::play::input_handler();
        let _ = bus.register_handler(target, policy, plan);
        // The project lifecycle (Ch.21 §21.21, WP-U7): create, open, save, push, pull and
        // build are session commands the core performs; promote, link-a-remote and the
        // project load are ordinary commands. Promote is the core's
        // own (`with_host`): it plans with the open project's presets (WP-16).
        for (target, policy, plan) in crate::project::handlers() {
            if target != lifecycle::PROMOTE_CMD {
                let _ = bus.register_handler(target, policy, plan);
            }
        }
        // The security class (Ch.21 §21.18, WP-U9): grants, revokes, the default automation
        // session policy, approve / reject (human-only), and the plugin set.
        for (target, policy, plan) in security::handlers(security::epoch()) {
            let _ = bus.register_handler(
                target,
                policy,
                move |b: &mut forge_cmd::DiffBuilder<'_>, a: &serde_json::Value| plan(b, a),
            );
        }
        // Scene composition (Ch.28 §28, WP-U20): the forge.scene.* commands and the deriver
        // that carries a base scene's edits to its instances in the same diff. The whole-
        // document loads (open, pull, a team sync) bring their own composed entities.
        forge_scene::install(bus, &crate::composition::WHOLE_DOCUMENT_LOADS);
        // Teams and sandboxes (Ch.37, WP-U10): team management, claims, publish and review,
        // pull and resolve, the review rules, and the core's own bind and sync.
        for (target, policy, plan) in crate::collab::handlers() {
            let _ = bus.register_handler(
                target,
                policy,
                move |b: &mut forge_cmd::DiffBuilder<'_>, a: &serde_json::Value| plan(b, a),
            );
        }
    }

    /// A core over `bus` (handlers registered, a test clock, a loaded project), with the
    /// first-party project host (`file:` and `memory:` stores, the in-memory packager).
    pub fn with_bus(bus: Bus) -> SharedCore {
        let host = ProjectHost::first_party().unwrap_or_else(|e| {
            // The store plugins did not load: every lifecycle command then reports why,
            // instead of the editor failing to start.
            let why = e.to_string();
            ProjectHost::new(
                Arc::new(move |_: &str, _: &str| Err(ProjectError::BadFiles(why.clone()))),
                Box::new(forge_project::packager::MemoryPackager),
            )
        });
        Self::with_host(bus, host)
    }

    /// A core over `bus` with `host` (tests inject stores and packagers here).
    pub fn with_host(bus: Bus, host: ProjectHost) -> SharedCore {
        Self::with_host_reserving(bus, host, true)
    }

    fn with_host_reserving(mut bus: Bus, host: ProjectHost, collab: bool) -> SharedCore {
        // The grant table follows `security.*`: only the security commands (and the core's
        // project load) may write it, on whatever bus the core runs (Ch.21 §21.18).
        for (prefix, writers) in security::RESERVED_SETTINGS {
            bus.reserve_settings(prefix, writers);
        }
        // Team binding and the review rules are reserved the same way (WP-U10); the core's
        // baseline sync writes whatever the baseline holds, security state included.
        if collab {
            for (prefix, writers) in crate::collab::RESERVED_SETTINGS {
                bus.reserve_settings(prefix, writers);
            }
        }
        Self::assemble(bus, host)
    }

    /// W2 positive control only: the editor's core with the security settings reserved but
    /// not the collaboration table (`team.*`, `collab.*`), so a plain `SetSetting` rebinds
    /// the team or switches review off. `test_reserved_team_settings`' control shows its
    /// check then fails. A running core can never be unreserved; this builds a separate one.
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn collab_unreserved_for_control() -> SharedCore {
        let host = ProjectHost::first_party().unwrap_or_else(|e| {
            let why = e.to_string();
            ProjectHost::new(
                Arc::new(move |_: &str, _: &str| Err(ProjectError::BadFiles(why.clone()))),
                Box::new(forge_project::packager::MemoryPackager),
            )
        });
        Self::with_host_reserving(Self::editor_bus(), host, false)
    }

    /// W2 positive control only: the editor's core with the plain `forge.project.load`
    /// planner, which loads a document's automation grants like any other setting (before
    /// WP-20). the self-grant guard's control shows an automation session that wrote a project file
    /// then grants itself `Command(Destructive)` by opening it. A running core never has it.
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn load_keeps_automation_grants_for_control() -> SharedCore {
        let mut bus = Bus::new();
        // Registered first, so the editor's handlers keep it.
        let _ = bus.register_handler(
            lifecycle::LOAD_CMD,
            lifecycle::LOAD_POLICY,
            forge_project::format::plan_load,
        );
        Self::register_editor_handlers(&mut bus);
        Self::with_bus(bus)
    }

    /// W2 positive control only: the editor's core with its security settings *not*
    /// reserved, so any command may write `security.*`. `test_reserved_security_settings`'
    /// control shows an automation session's plain `SetSetting` then grants itself a capability. A
    /// running core can never be unreserved; this builds a separate one.
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn unreserved_for_control() -> SharedCore {
        let host = ProjectHost::new(
            Arc::new(|_: &str, _: &str| Err(ProjectError::BadFiles("control core".into()))),
            Box::new(forge_project::packager::MemoryPackager),
        );
        Self::assemble(Self::editor_bus(), host)
    }

    fn assemble(mut bus: Bus, mut host: ProjectHost) -> SharedCore {
        // Automation grants are per run (their value is this run's epoch): no save writes one.
        host.set_per_run_settings(Some(Arc::new(security::per_run_setting)));
        let presets = lifecycle::promote::builtin_preset_defaults();
        let rules: lifecycle::promote::SharedRules = Arc::default();
        // Every core composes scenes, whatever bus it was given (WP-U20).
        if !bus.has_deriver() {
            forge_scene::install(&mut bus, &crate::composition::WHOLE_DOCUMENT_LOADS);
        }
        // A bus that already has a promote planner (built without `editor_bus`) keeps it.
        let _ = bus.register_handler(
            lifecycle::PROMOTE_CMD,
            forge_cmd::CommandPolicy::ORDINARY,
            lifecycle::promote::promote_handler(Arc::clone(&presets), Arc::clone(&rules)),
        );
        let grants = SharedGrants::new();
        let mut sync = GrantSync::new(security::epoch());
        sync.resync(bus.project(), &grants);
        let watch = bus.subscribe();
        Arc::new(Mutex::new(EditorCore {
            presets,
            rules,
            bus,
            wakers: BTreeMap::new(),
            next_client: 1,
            session_targets: [
                crate::play::PLAY_CMD.to_string(),
                crate::play::PLAY_INPUT_CMD.to_string(),
            ]
            .into_iter()
            .chain(lifecycle::SESSION_TARGETS.iter().map(|t| t.to_string()))
            .collect(),
            inboxes: BTreeMap::new(),
            project: host,
            transfers: Vec::new(),
            faults: CoreFaults::default(),
            grants,
            sync,
            watch,
            audit: AuditBook::default(),
            collab: None,
            collab_watchers: Vec::new(),
            collab_gen: 0,
            wake_list: Arc::new(Mutex::new(BTreeMap::new())),
            next_automation_session: 1,
            held: None,
            next_held: 1,
            trust_book: Arc::new(crate::trust::MemoryTrust::default()),
            trust: None,
            next_trust: 1,
            trust_version: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            core_applied: std::collections::BTreeSet::new(),
            catalog: Arc::new(std::sync::RwLock::new(
                crate::presets::PresetCatalog::builtin(),
            )),
        }))
    }

    /// The one grant table (a handle: the WASM host and every session host share it).
    pub fn grants(core: &SharedCore) -> SharedGrants {
        lock(core).grants.clone()
    }

    /// The next automation session number for this core (from 1): every session host over the
    /// core — the editor's, a second one, one restarted — draws from this one count, so an
    /// `auto-N` is never a second session (its undo ownership, grants and audit lines).
    pub fn next_automation_session(core: &SharedCore) -> u64 {
        let mut c = lock(core);
        let n = c.next_automation_session;
        c.next_automation_session += 1;
        n
    }

    /// The core's audit book (security commands, approvals; the remote store writes to it
    /// too).
    pub fn audit_book(core: &SharedCore) -> AuditBook {
        lock(core).audit.clone()
    }

    /// Use `book` as the core's audit book (a test clock; one book shared with a remote
    /// store made first).
    pub fn set_audit_book(core: &SharedCore, book: AuditBook) {
        lock(core).audit = book;
    }

    /// A transaction's record and its changes so far (the session-sessions panel's diff view
    /// of a pending preview transaction). Read-only.
    pub fn txn_changes(core: &SharedCore, txn: TxnId) -> Option<(TxnInfo, Vec<forge_cmd::Change>)> {
        let c = lock(core);
        c.bus
            .transaction(txn)
            .map(|r| (info(r), r.changes().cloned().collect()))
    }

    /// The open transactions of automation issuers, oldest first (the session-sessions panel's
    /// pending list, whatever a session host last reported).
    pub fn open_automation_txns(core: &SharedCore) -> Vec<TxnInfo> {
        let mut v: Vec<TxnInfo> = lock(core)
            .bus
            .open_transactions()
            .filter(|r| matches!(r.issuer(), Issuer::Automation { .. }))
            .map(info)
            .collect();
        v.sort_by_key(|i| i.txn);
        v
    }

    /// Install plugin `Command` items (a WASM plugin's, loaded through the ordinary loader)
    /// as `Invoke` targets on the core: core configuration, like [`EditorCore::configure`].
    pub fn install_commands(
        core: &SharedCore,
        reg: &forge_plugin::Registry<forge_plugin::points::Command>,
    ) -> Result<usize, forge_cmd::CmdError> {
        forge_plugin::points::install_commands(reg, &mut lock(core).bus)
    }

    /// Install the plugin commands of `reg` that the core does not run yet (the editor's
    /// hosting adds a WASM plugin dropped in while it runs, WP-21): a target already on the
    /// bus is left as it is. Returns how many were added.
    pub fn install_new_commands(
        core: &SharedCore,
        reg: &forge_plugin::Registry<forge_plugin::points::Command>,
    ) -> Result<usize, forge_cmd::CmdError> {
        let mut c = lock(core);
        let have: BTreeSet<String> = c.bus.targets().map(str::to_string).collect();
        let mut fresh = forge_plugin::Registry::<forge_plugin::points::Command>::new();
        for (k, item) in reg.iter().filter(|(k, _)| !have.contains(*k)) {
            let owner = reg.provenance(k).map(|p| p.owner.clone()).ok_or_else(|| {
                forge_cmd::CmdError::BadArgs {
                    target: k.to_string(),
                    why: "no provenance".into(),
                }
            })?;
            fresh
                .add(owner, k, item.clone(), forge_plugin::Order::Last)
                .map_err(|e| forge_cmd::CmdError::BadArgs {
                    target: k.to_string(),
                    why: e.to_string(),
                })?;
        }
        forge_plugin::points::install_commands(&fresh, &mut c.bus)
    }

    /// Install every plugin command of `reg` or none (a live install's last commit step,
    /// WP-34): refused, with nothing registered, when the core already runs any of their
    /// targets. Checked and registered under one lock, so nothing can slip in between.
    pub fn install_commands_all(
        core: &SharedCore,
        reg: &forge_plugin::Registry<forge_plugin::points::Command>,
    ) -> Result<usize, forge_cmd::CmdError> {
        let mut c = lock(core);
        if let Some(t) = reg.keys().find(|k| c.bus.targets().any(|x| x == *k)) {
            return Err(forge_cmd::CmdError::DuplicateHandler(t.to_string()));
        }
        forge_plugin::points::install_commands(reg, &mut c.bus)
    }

    /// W2 positive control only: the core stops following grants into the table (they are
    /// still recorded in the project). `test_security_commands`' control shows a grant made in
    /// the panel then never reaches an automation session.
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn disable_grant_sync_for_control(core: &SharedCore) {
        lock(core).sync.disabled.on = true;
    }

    /// The project lifecycle's status (the headless summary, tests).
    pub fn project_status(core: &SharedCore) -> Arc<ProjectStatus> {
        lock(core).project.status()
    }

    /// Connect a client issuing as `issuer`. Its subscription starts now.
    pub fn connect(core: &SharedCore, issuer: Issuer) -> LocalBus {
        let mut c = lock(core);
        let id = c.next_client;
        c.next_client += 1;
        let sub = c.bus.subscribe();
        drop(c);
        LocalBus {
            core: core.clone(),
            id,
            issuer,
            sub,
            refused: Vec::new(),
            next_ticket: 1,
            seen_project: None,
        }
    }

    /// Read the core (status lines, the headless summary). Never a way to change it.
    pub fn read<R>(core: &SharedCore, f: impl FnOnce(&Project) -> R) -> R {
        f(lock(core).bus.project())
    }

    /// W2 positive controls only: break a guarded property of the core (see [`CoreFaults`]).
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn set_faults(core: &SharedCore, faults: CoreFaults) {
        let mut c = lock(core);
        if faults.save_keeps_per_run() {
            c.project.set_per_run_settings(None);
        }
        c.faults = faults;
        c.trust_moved();
    }

    /// Set up the core before clients use it: register `Invoke` handlers and `#[forge_api]`
    /// commands. This is core configuration (the plugin loader's job), not an edit.
    pub fn configure<R>(core: &SharedCore, f: impl FnOnce(&mut Bus) -> R) -> R {
        f(&mut lock(core).bus)
    }

    /// A client changed the project: bring the grant table and the audit book up to date
    /// (before the lock is released, so no client sees a grant the table does not hold),
    /// then wake the other clients.
    fn notify_others(&mut self, me: u64) {
        self.follow_security();
        for (id, w) in &self.wakers {
            if *id != me {
                w();
            }
        }
    }

    /// Follow the core's own view of the `Applied` stream: the grant table, the audit book,
    /// and the team sandbox's transaction counts (the bus-side undo / redo / cancel hook,
    /// ADR 0041). Every path that changes the bus reaches here before the lock is released
    /// or the sandbox is read, so no call site has to remember to report a transaction.
    fn follow_security(&mut self) {
        let d = self.watch.drain();
        if d.gap.is_some() {
            self.collab_resync_txns();
            // More changes than the watch holds: follow the whole project instead.
            self.sync.resync(self.bus.project(), &self.grants);
            self.audit.record(
                AuditOrigin::Security,
                "",
                "",
                "resync",
                "the grant table was rebuilt from the project after a stream gap",
            );
            // What the gap dropped is unknown: nothing of it moves the person's trust answer.
            self.core_applied.clear();
        }
        let mut own = (std::collections::BTreeSet::new(), false);
        for a in d.events.iter() {
            self.collab_follow_applied(a);
            let events = self
                .sync
                .apply(&a.diff.changes, self.bus.project(), &self.grants);
            for e in events {
                self.audit_security(a, e);
            }
            if !self.core_applied.remove(&a.seq) && a.issuer.is_human() {
                for c in &a.diff.changes {
                    if let forge_cmd::Change::Setting { key, .. } = c {
                        if crate::trust::is_policy_setting(key) {
                            own.1 = true;
                        } else if let Some(item) = crate::trust::item_of_setting(key) {
                            own.0.insert(item);
                        }
                    }
                }
            }
        }
        if !own.0.is_empty() || own.1 {
            self.follow_own_trust(&own.0, own.1);
        }
    }

    /// A person changed plugin grants, the automation policy or the plugin set with a command of
    /// their own (WP-35): when they trust the open project, their answer follows — the items
    /// they changed are recorded as they are now (a grant they made, a plugin they added, at
    /// the version they chose; a revoke or a removal drops it) — so their own change never
    /// asks them. A load or a team pull never gets here ([`EditorCore::core_applied`]): what
    /// it brings the person has not seen.
    fn follow_own_trust(&mut self, items: &std::collections::BTreeSet<String>, policy: bool) {
        let Some(location) = self.project.location().map(str::to_string) else {
            return;
        };
        let key = crate::trust::trust_key(&location);
        let Some(mut rec) = self.trust_book.get(&key) else {
            return;
        };
        if rec.trust != crate::trust::Trust::Trusted {
            return;
        }
        let now: BTreeMap<String, crate::trust::CarriedItem> = crate::trust::carried_settings(
            &crate::trust::security_settings(self.bus.project().settings()),
        )
        .into_iter()
        .map(|i| (i.item.clone(), i))
        .collect();
        let mut question = self
            .trust
            .as_ref()
            .map(|t| t.items.clone())
            .unwrap_or_default();
        let mut follow = |item: &str, rec: &mut crate::trust::TrustRecord| match now.get(item) {
            Some(it) => {
                rec.content.insert(item.to_string(), it.digest.clone());
                question.insert(item.to_string(), it.clone());
            }
            None => {
                rec.content.remove(item);
                question.remove(item);
            }
        };
        for item in items {
            follow(item, &mut rec);
        }
        if policy {
            let old: Vec<String> = rec
                .content
                .keys()
                .filter(|k| crate::trust::is_policy_setting(k))
                .cloned()
                .collect();
            let new: Vec<String> = now
                .keys()
                .filter(|k| crate::trust::is_policy_setting(k))
                .cloned()
                .collect();
            for item in old.iter().chain(new.iter()) {
                follow(item, &mut rec);
            }
        }
        if let Err(e) = self.record_trust(&key, rec) {
            self.audit.record(
                AuditOrigin::Security,
                "",
                "",
                "trust",
                format!("{key}: the person's own change was not remembered: {e}"),
            );
        }
        self.set_question(&key, question);
    }

    fn audit_security(&self, a: &forge_cmd::Applied, e: SecurityEvent) {
        let user = AuditRecord::user_of(&a.issuer);
        let how = match a.kind {
            forge_cmd::AppliedKind::Undo => " (by undo)",
            forge_cmd::AppliedKind::Redo => " (by redo)",
            forge_cmd::AppliedKind::Cancel => " (cancelled)",
            _ => "",
        };
        let (session, event, detail) = match e {
            SecurityEvent::Granted { who, cap } => (
                session_of(&who),
                "grant",
                format!("{cap} to {who}{how} ({})", a.txn),
            ),
            SecurityEvent::Revoked { who, cap } => (
                session_of(&who),
                "revoke",
                format!("{cap} from {who}{how} ({})", a.txn),
            ),
            SecurityEvent::Policy { caps } => (
                String::new(),
                "automation_policy",
                format!(
                    "default automation capabilities: {}{how}",
                    caps.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
            SecurityEvent::PluginSet { plugin, what } => {
                (String::new(), "plugin_set", format!("{plugin} {what}{how}"))
            }
        };
        self.audit
            .record(AuditOrigin::Security, &session, &user, event, detail);
    }

    /// An approve / reject is valid only for an automation session's open preview transaction;
    /// checked under the lock the command then applies under.
    fn check_review(&self, args: &str) -> Result<(TxnId, Issuer), Rejection> {
        let refuse =
            |what: String| Rejection::new(forge_cmd::CmdError::PolicyRefused { what }, None, None);
        let txn = security::review_txn(args)
            .ok_or_else(|| refuse("an approval names a transaction".into()))?;
        let Some(rec) = self.bus.transaction(txn) else {
            // Closed and retired (cancelled, committed empty, expired), or never seen.
            return Err(match self.bus.txn_state(txn) {
                Some(found) => Rejection::new(
                    forge_cmd::CmdError::TxnState {
                        txn,
                        expected: TxnState::Open,
                        found,
                    },
                    None,
                    Some(txn),
                ),
                None => refuse(format!("{txn} is not pending")),
            });
        };
        if rec.state() != TxnState::Open {
            return Err(Rejection::new(
                forge_cmd::CmdError::TxnState {
                    txn,
                    expected: TxnState::Open,
                    found: rec.state(),
                },
                None,
                Some(txn),
            ));
        }
        if !matches!(rec.issuer(), Issuer::Automation { .. }) {
            return Err(refuse(format!(
                "{txn} is {}'s, not an automation session's preview transaction",
                rec.issuer()
            )));
        }
        Ok((txn, rec.issuer().clone()))
    }

    /// Commit (approve) or cancel (reject) the session's transaction the accepted command
    /// names, and write the audit record.
    fn perform_review(
        &mut self,
        target: &str,
        txn: TxnId,
        auto: &Issuer,
        by: &Issuer,
    ) -> Result<(), Rejection> {
        let approve = target == security::AUTOMATION_APPROVE_CMD;
        let changes = self.bus.transaction(txn).map_or(0, |r| r.changes().count());
        let r = if approve {
            self.bus.commit(txn)
        } else {
            let a = self.bus.cancel_as(txn, auto)?;
            self.changed(&a);
            Ok(a)
        };
        let session = match auto {
            Issuer::Automation { session, .. } => session.clone(),
            other => other.tag(),
        };
        self.audit.record(
            AuditOrigin::Security,
            &session,
            &AuditRecord::user_of(by),
            if approve { "approve" } else { "reject" },
            format!(
                "{txn} of {auto}: {changes} change(s) {}{}",
                if approve { "committed" } else { "reverted" },
                match &r {
                    Ok(_) => String::new(),
                    Err(e) => format!(" \u{2014} failed: {e}"),
                }
            ),
        );
        r.map(drop)
    }

    /// Replace the project with `doc` (open, create, pull): one `forge.project.load`
    /// command issued by the requester — atomic, audited, streamed to every client — then
    /// a fresh undo history (the previous project's transactions mean nothing here).
    ///
    /// **A non-human's load never widens security (WP-33).** Plugin grants and the default
    /// automation policy ship with a project, so a load writes them — but when an automation
    /// session or a script started it (an automation session a human let write files could have
    /// written them), a plugin grant or a default automation capability the open project does not
    /// have is held back: the load writes what is in effect now instead
    /// ([`security::held_changes`]), and the core keeps the file's values as a proposal a human
    /// accepts or discards in the Automation sessions panel (`forge.security.accept_held` /
    /// `discard_held`, human-only). A restriction takes effect. A human's load is unchanged.
    /// Returns how many settings were held.
    ///
    /// **A project nobody trusted grants nothing (WP-34, [`crate::trust`]).** Whoever opened
    /// it, a project the person has not trusted has its plugin grants held — against an empty
    /// project, so no grant of it takes effect however the project before it granted — and
    /// so is a default automation policy wider than the standard one (the WP-34 verifier's
    /// follow-up: it would widen what every automation session may do), until the person trusts it
    /// ([`EditorCore::decide_trust`]) or accepts them.
    ///
    /// **Only what the person trusted (WP-35).** A trusted project that carries something new
    /// or changed since ([`crate::trust::TrustState::Changed`]: a `git pull`, another
    /// repository cloned into the folder) holds what the person did not trust: its new plugin
    /// grants and wider automation capabilities, against what the person trusted of it
    /// ([`crate::trust::approved_settings`]) — what they trusted takes effect as before.
    fn load(
        &mut self,
        doc: &ProjectDoc,
        issuer: &Issuer,
        source: &str,
    ) -> Result<usize, ProjectError> {
        let location = self.project.location().unwrap_or(source).to_string();
        let key = crate::trust::trust_key(&location);
        let carried = crate::trust::carried(&location, &doc.settings);
        let content: crate::trust::Content = carried
            .iter()
            .map(|i| (i.item.clone(), i.digest.clone()))
            .collect();
        let baseline = match self.trust_state(&key, &content) {
            crate::trust::TrustState::Trusted => None,
            crate::trust::TrustState::Changed => Some(crate::trust::approved_settings(
                &self
                    .trust_book
                    .get(&key)
                    .map(|r| r.content)
                    .unwrap_or_default(),
            )),
            _ => Some(BTreeMap::new()),
        };
        let (held, untrusted) = self.held_for(issuer, &doc.settings, baseline.as_ref());
        let narrowed = self.narrowed_by(issuer, &doc.settings);
        let cmd = if held.is_empty() {
            doc.load_command()?
        } else {
            let mut safe = doc.clone();
            security::apply_held(&mut safe.settings, &held);
            safe.load_command()?
        };
        let e = self.bus.envelope(issuer.clone(), cmd);
        let applied = self
            .bus
            .apply(e)
            .map_err(|r| ProjectError::BadFiles(format!("the project was refused: {}", r.error)))?;
        // What the load brings is the files', not the person's own change (WP-35).
        self.core_applied.insert(applied.seq);
        self.bus.clear_history();
        self.project.loaded();
        // Another project (or the same one reloaded) is open: the gate's answers may differ.
        self.trust_moved();
        self.follow_presets();
        // The proposal belonged to the project this load replaced.
        self.set_held(None);
        self.audit_narrowed(issuer, source, &narrowed);
        self.set_question(
            &key,
            carried.into_iter().map(|i| (i.item.clone(), i)).collect(),
        );
        Ok(self.hold(issuer, source, held, untrusted))
    }

    /// What a load or a team pull of `settings` holds for a person, and whether trust is why
    /// (some of it): a non-human's widenings against the project in memory (WP-33), and — when
    /// the project is not trusted (or changed since it was, WP-35), `untrusted_baseline` being
    /// what its grants are compared with — every plugin grant and default automation capability
    /// it would add (WP-34, [`security::held_changes`] against the baseline). An item both rules
    /// hold is held as the trust rule has it (the stricter: what the baseline grants). Sorted by
    /// key.
    fn held_for(
        &self,
        issuer: &Issuer,
        settings: &BTreeMap<String, Value>,
        untrusted_baseline: Option<&BTreeMap<String, Value>>,
    ) -> (Vec<security::HeldSetting>, bool) {
        let mut held = match untrusted_baseline {
            Some(b) if !self.faults.untrusted_load_unheld() => {
                security::held_changes_by(|k| b.get(k), settings)
            }
            _ => Vec::new(),
        };
        let untrusted = !held.is_empty();
        if !(issuer.is_human() || self.faults.automation_load_unheld()) {
            for h in security::held_changes(self.bus.project(), settings) {
                if !held.iter().any(|x| x.key == h.key) {
                    held.push(h);
                }
            }
        }
        held.sort_by(|a, b| a.key.cmp(&b.key));
        (held, untrusted)
    }

    /// Where the project `key` stands, carrying `content` (see [`crate::trust`]).
    pub(crate) fn trust_state(
        &self,
        key: &str,
        content: &crate::trust::Content,
    ) -> crate::trust::TrustState {
        if self.faults.trust_by_folder() {
            // The fault: the folder's answer covers whatever it carries (before WP-35).
            let folder = self
                .trust_book
                .get(key)
                .map(|r| crate::trust::TrustRecord::new(r.trust, content.clone()));
            return crate::trust::TrustState::of(folder.as_ref(), content);
        }
        self.trust_book.state(key, content)
    }

    /// Whether the person trusted `item` of the open project as it is now. With no project
    /// open the editor holds the person's own unsaved work, which came from no one else.
    fn trust_approves(&self, item: &crate::trust::CarriedItem) -> bool {
        let Some(location) = self.project.location() else {
            return true;
        };
        let key = crate::trust::trust_key(location);
        if self.faults.trust_by_folder() {
            return self
                .trust_book
                .get(&key)
                .is_some_and(|r| r.trust == crate::trust::Trust::Trusted);
        }
        self.trust_book.approves(&key, &item.item, &item.digest)
    }

    /// The open project's trust question: `items` is what it carries (none: no question).
    /// The id changes when anything the person reads does.
    pub(crate) fn set_question(
        &mut self,
        key: &str,
        items: BTreeMap<String, crate::trust::CarriedItem>,
    ) {
        if items.is_empty() {
            self.trust = None;
            return;
        }
        let content: crate::trust::Content = items
            .iter()
            .map(|(k, i)| (k.clone(), i.digest.clone()))
            .collect();
        let state = self.trust_state(key, &content);
        let approved = self.trust_book.get(key).map(|r| r.content);
        let mut q = crate::trust::ProjectTrust::new(0, key, state, items, approved.as_ref());
        if let Some(t) = &self.trust {
            q.id = t.id;
            if *t == q {
                return;
            }
        }
        q.id = self.next_trust;
        self.next_trust += 1;
        self.trust = Some(q);
    }

    /// Recompute where the open project's question stands (the book or its answers changed).
    fn refresh_question(&mut self) {
        if let Some(t) = self.trust.clone() {
            self.set_question(&t.project, t.items);
        }
    }

    /// The hosting found plugins of the open project that it did not load (or whose changed
    /// code it did not reload) because the person has not trusted them as they are now (they
    /// may have arrived after the load, e.g. by a `git pull` in a terminal): list them in what
    /// the person is asked about.
    fn note_carried(&mut self, found: &[crate::trust::CarriedItem]) {
        let Some(location) = self.project.location().map(str::to_string) else {
            return;
        };
        let key = crate::trust::trust_key(&location);
        let mut items = self
            .trust
            .as_ref()
            .filter(|t| t.project == key)
            .map(|t| t.items.clone())
            .unwrap_or_default();
        for it in found {
            items.insert(it.item.clone(), it.clone());
        }
        self.set_question(&key, items);
    }

    /// Remember the person's answers in `book` (the GUI: the user config file; headless
    /// `--trust-project`: [`crate::trust::TrustEvery`]). The open project's state follows.
    pub fn set_trust_book(core: &SharedCore, book: Arc<dyn crate::trust::TrustBook>) {
        let mut c = lock(core);
        c.trust_book = book;
        c.trust_moved();
        c.refresh_question();
    }

    /// What moves whenever an answer of the plugin gate ([`EditorCore::vet_carried`]) could
    /// change (WP-36): a reader that caches the gate's answers keeps them while it stands
    /// still, without taking the core's lock. Read it before asking the gate.
    pub fn trust_version(core: &SharedCore) -> Arc<std::sync::atomic::AtomicU64> {
        Arc::clone(&lock(core).trust_version)
    }

    /// An answer of the plugin gate may have changed (see [`EditorCore::trust_version`]).
    fn trust_moved(&self) {
        self.trust_version
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    /// Record `record` for the project `key` in the trust book (every answer goes through
    /// here, so the gate's version follows it).
    fn record_trust(&self, key: &str, record: crate::trust::TrustRecord) -> Result<(), String> {
        let r = self.trust_book.set(key, record);
        self.trust_moved();
        r
    }

    /// Where the person's trust answers are remembered (the Plugin manager's line).
    pub fn trust_book_describe(core: &SharedCore) -> String {
        lock(core).trust_book.describe()
    }

    /// What the open project carries that needs trust, and where it stands (`None`: it
    /// carries nothing to run or grant; see [`crate::trust`]).
    pub fn project_trust(core: &SharedCore) -> Option<crate::trust::ProjectTrust> {
        lock(core).trust.clone()
    }

    /// Where the open project lives (`None`: no project is open).
    pub fn read_location(core: &SharedCore) -> Option<String> {
        lock(core).project.location().map(str::to_string)
    }

    /// The id of [`EditorCore::project_trust`], if any: what a panel follows without cloning.
    pub fn project_trust_id(core: &SharedCore) -> Option<u64> {
        lock(core).trust.as_ref().map(|t| t.id)
    }

    /// The plugin gate (WP-34, keyed by content since WP-35; the hosting asks at start and at
    /// each poll): for each plugin the open project brings — a `plugins/<name>` folder with
    /// the digest of its manifest and code, a cached plugin at the version its set names —
    /// whether the person trusted it as it is now. The ones they did not are added to the
    /// question the person is asked, under the same lock.
    pub fn vet_carried(core: &SharedCore, items: &[crate::trust::CarriedItem]) -> Vec<bool> {
        let mut c = lock(core);
        let ok: Vec<bool> = items.iter().map(|i| c.trust_approves(i)).collect();
        let refused: Vec<crate::trust::CarriedItem> = items
            .iter()
            .zip(&ok)
            .filter(|(_, ok)| !**ok)
            .map(|(i, _)| i.clone())
            .collect();
        if !refused.is_empty() {
            c.note_carried(&refused);
        }
        ok
    }

    /// The person's answer about the open project (WP-34, [`crate::trust`]): remembered in
    /// the trust book (user config — never the project, and never a bus command, so no
    /// automation session, script or remote client can give it) with what the project carries now
    /// (WP-35: a later change asks again), and audited as `trust` / `distrust`.
    /// Trusting also accepts the plugin grants the gate held when a person's load held them
    /// (as `user`, through the human-only `forge.security.accept_held`, I7); what an automation
    /// session or a script opened stays held for a separate look (ADR 0043 Amendment 1). The
    /// project's plugins install (and changed code reloads) at the hosting's next poll. The line to
    /// show, or why it failed.
    pub fn decide_trust(
        core: &SharedCore,
        user: &str,
        trust: crate::trust::Trust,
    ) -> Result<String, String> {
        Self::decide_trust_on(core, user, trust, None)
    }

    /// [`EditorCore::decide_trust`] about the question the person was shown: `seen` is its
    /// id ([`crate::trust::ProjectTrust::id`]), and an answer to a question that changed
    /// since (a pull brought something new between the showing and the answer) is refused,
    /// under the same lock as the answer is recorded — the `--remote-host` console's `trust` /
    /// `distrust ID` (WP-36).
    pub fn decide_trust_seen(
        core: &SharedCore,
        user: &str,
        trust: crate::trust::Trust,
        seen: u64,
    ) -> Result<String, String> {
        Self::decide_trust_on(core, user, trust, Some(seen))
    }

    fn decide_trust_on(
        core: &SharedCore,
        user: &str,
        trust: crate::trust::Trust,
        seen: Option<u64>,
    ) -> Result<String, String> {
        let (line, accept) = {
            let mut c = lock(core);
            let location = c
                .project
                .location()
                .map(str::to_string)
                .ok_or(forge_ui::tr!("no project is open"))?;
            let Some(question) = c.trust.clone() else {
                // An answer about nothing would still be remembered and apply to whatever the
                // folder carries later: only a question the person saw gets an answer.
                return Err(forge_ui::tr!(
                    "the open project carries no plugins, plugin grants or wider automation capabilities: nothing to trust"
                )
                .into());
            };
            if let Some(id) = seen
                && id != question.id
            {
                return Err(forge_ui::trf!(
                    "the project's trust question is #{now} now, not #{id}: it changed since it was shown; look at it again",
                    now = question.id,
                    id
                ));
            }
            let key = crate::trust::trust_key(&location);
            let was_trusted = c
                .trust_book
                .get(&key)
                .is_some_and(|r| r.trust == crate::trust::Trust::Trusted);
            let remembered = c.record_trust(
                &key,
                crate::trust::TrustRecord::new(trust, question.content()),
            );
            c.set_question(&key, question.items.clone());
            let trusted = trust == crate::trust::Trust::Trusted;
            let what = question.lines().join("; ");
            c.audit.record(
                AuditOrigin::Security,
                "",
                user,
                if trusted { "trust" } else { "distrust" },
                format!(
                    "{user} {} {key}: {what}",
                    if trusted { "trusted" } else { "did not trust" }
                ),
            );
            let accept = c
                .held
                .as_ref()
                .filter(|h| trusted && h.untrusted && h.by.starts_with("human:"))
                .map(|h| h.id);
            let line = format!(
                "{} {key}{}{}",
                if trusted { "Trusted" } else { "Not trusted:" },
                if was_trusted && !trusted {
                    " (plugins of it already running stay until the editor restarts)"
                } else {
                    ""
                },
                match remembered {
                    Ok(()) => String::new(),
                    Err(e) => format!(" (for this run only: the answer was not remembered: {e})"),
                }
            );
            (line, accept)
        };
        match accept {
            Some(id) => Self::answer_held(core, user, id, true).map(|a| format!("{line}; {a}")),
            None => Ok(line),
        }
    }

    /// What a non-human's load or team pull of `settings` restricts (plugin grants dropped,
    /// default automation capabilities turned off): empty for a person's, whose decision it is.
    fn narrowed_by(&self, issuer: &Issuer, settings: &BTreeMap<String, Value>) -> Vec<String> {
        if issuer.is_human() || self.faults.narrowed_unaudited() {
            return Vec::new();
        }
        security::narrowed_changes(self.bus.project(), settings)
    }

    /// Audit restrictions a non-human's load or pull made as one `narrowed` record (WP-21):
    /// they took effect — they only make things safer — but a person can see what an automation
    /// session or a script took away, and restore it with a grant.
    fn audit_narrowed(&mut self, issuer: &Issuer, source: &str, narrowed: &[String]) {
        if narrowed.is_empty() {
            return;
        }
        self.audit.record(
            AuditOrigin::Security,
            &issuer_session(issuer),
            &AuditRecord::user_of(issuer),
            "narrowed",
            format!(
                "{} opened {source}: {} security setting(s) restricted: {}",
                issuer.tag(),
                narrowed.len(),
                narrowed.join("; ")
            ),
        );
    }

    /// Keep `held` (what a non-human's load or pull did not let take effect) as the proposal
    /// a person answers, audited as `held`. Returns how many settings were held.
    fn hold(
        &mut self,
        issuer: &Issuer,
        source: &str,
        held: Vec<security::HeldSetting>,
        untrusted: bool,
    ) -> usize {
        let n = held.len();
        if n > 0 {
            let h = security::HeldSecurity {
                id: self.next_held,
                by: issuer.tag(),
                source: source.to_string(),
                items: held,
                untrusted,
            };
            self.next_held += 1;
            self.audit.record(
                AuditOrigin::Security,
                &issuer_session(issuer),
                &AuditRecord::user_of(issuer),
                "held",
                format!(
                    "{}: {}",
                    h.label(),
                    h.items
                        .iter()
                        .map(security::HeldSetting::line)
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            );
            self.set_held(Some(h));
        }
        n
    }

    /// Hold `h` (or nothing) for a person, and tell the project host to write its settings as
    /// the project's files hold them until the person answers: a save or a build by anyone in
    /// the meantime leaves the file's plugin grants and default automation policy as they are, so
    /// no commit reverts a teammate's grants and a restart finds them still on disk (WP-33).
    fn set_held(&mut self, h: Option<security::HeldSecurity>) {
        let keep = match &h {
            Some(h) if !self.faults.save_writes_held() => h
                .items
                .iter()
                .map(|i| forge_project::host::FileSetting {
                    key: i.key.clone(),
                    file: i.file.clone(),
                    loaded: i.in_effect.clone(),
                })
                .collect(),
            _ => Vec::new(),
        };
        self.project.set_file_settings(keep);
        self.held = h;
    }

    /// An accept / discard of held security settings is valid only for the proposal the
    /// core holds, and an accept only when it writes exactly what was held; checked under
    /// the lock the command then applies under.
    fn check_held(&self, target: &str, args: &str) -> Result<u64, Rejection> {
        let refuse =
            |what: String| Rejection::new(forge_cmd::CmdError::PolicyRefused { what }, None, None);
        let id = security::held_id(args)
            .ok_or_else(|| refuse(format!("{target} names held settings by \"id\"")))?;
        let h = self
            .held
            .as_ref()
            .filter(|h| h.id == id)
            .ok_or_else(|| refuse(format!("no security settings are held as #{id}")))?;
        if target == security::ACCEPT_HELD_CMD && !security::accept_matches(args, h) {
            return Err(refuse(format!(
                "{target} must write exactly the settings held as #{id}"
            )));
        }
        Ok(id)
    }

    /// Forget the held proposal the accepted command answered, and audit the answer.
    fn perform_held(&mut self, target: &str, id: u64, by: &Issuer) {
        let Some(h) = self.held.take_if(|h| h.id == id) else {
            return;
        };
        // Answered: from now on the project as it is in memory is what a save writes (an
        // accept wrote the file's values; a discard keeps what took effect).
        self.set_held(None);
        let accept = target == security::ACCEPT_HELD_CMD;
        let session =
            h.by.strip_prefix("automation:")
                .map(str::to_string)
                .unwrap_or_default();
        self.audit.record(
            AuditOrigin::Security,
            &session,
            &AuditRecord::user_of(by),
            if accept {
                "accept_held"
            } else {
                "discard_held"
            },
            format!(
                "{} security setting(s) {} opened from {} {}",
                h.items.len(),
                h.by,
                h.source,
                if accept { "accepted" } else { "discarded" }
            ),
        );
    }

    /// The security settings a non-human's load held back, if any (the Automation sessions
    /// panel's list; see the core's project load).
    pub fn held_security(core: &SharedCore) -> Option<security::HeldSecurity> {
        lock(core).held.clone()
    }

    /// The id of the held proposal, if any: what a panel follows without cloning it.
    pub fn held_security_id(core: &SharedCore) -> Option<u64> {
        lock(core).held.as_ref().map(|h| h.id)
    }

    /// A person's answer to the held proposal `id` (WP-21: the `--remote-host` console; the
    /// panels emit the same commands): `forge.security.accept_held` (or `discard_held`) sent
    /// as `Human { user }` through an ordinary client, so the bus's human-only policy, the
    /// core's check against the proposal it holds, and the audit all apply. The line to show,
    /// or why it was refused.
    pub fn answer_held(
        core: &SharedCore,
        user: &str,
        id: u64,
        accept: bool,
    ) -> Result<String, String> {
        let h = Self::held_security(core)
            .filter(|h| h.id == id)
            .ok_or_else(|| match Self::held_security_id(core) {
                Some(now) => format!("#{id} is not held (#{now} is)"),
                None => "no security settings are held".to_string(),
            })?;
        let mut person = Self::connect(core, Issuer::Human { user: user.into() });
        person.apply(
            if accept {
                security::accept_held_command(&h)
            } else {
                security::discard_held_command(&h)
            },
            None,
        );
        match person.pump().refused.first() {
            Some(r) => Err(r.rejection.to_string()),
            None => Ok(format!(
                "{} {} held security setting(s) (#{id}, from {} opening {})",
                if accept { "accepted" } else { "discarded" },
                h.items.len(),
                h.by,
                h.source
            )),
        }
    }

    /// Create new projects from `catalog` (the editor's real `Preset` registry, WP-21): the
    /// shell that hosts the plugins shares it, so a preset plugin installed while the editor
    /// runs is offered and created from at once.
    pub fn set_presets(core: &SharedCore, catalog: crate::presets::SharedPresets) {
        let mut c = lock(core);
        c.catalog = catalog;
        // A plugin's preset (`forge.preset.<dir>`) is promoted to with its defaults.
        c.follow_presets();
    }

    /// Plan promotions with `rules` (the plugins' `PromotionRule` registry).
    pub fn set_promotion_rules(core: &SharedCore, rules: lifecycle::promote::PromotionRules) {
        let c = lock(core);
        *c.rules.write().unwrap_or_else(PoisonError::into_inner) = rules;
    }

    /// The preset catalog new projects are created from.
    pub fn presets(core: &SharedCore) -> crate::presets::SharedPresets {
        Arc::clone(&lock(core).catalog)
    }

    /// Use the catalog's presets, with the open project's own copies (its `presets/`, WP-16)
    /// in their place, for promotion; a copy that does not load leaves the catalog's one (the
    /// panel names why).
    fn follow_presets(&mut self) {
        let status = self.project.status();
        let files = status
            .open
            .as_ref()
            .map(|o| o.presets.clone())
            .unwrap_or_default();
        let catalog = self
            .catalog
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let set = crate::presets::PresetSet::with_project_in(&catalog, &files)
            .or_else(|_| crate::presets::PresetSet::builtin());
        if let Ok(set) = set {
            *self.presets.write().unwrap_or_else(PoisonError::into_inner) = set.defaults();
        }
    }

    /// An undo, redo or cancel applied: a change is unsaved work.
    fn changed(&mut self, a: &forge_cmd::Applied) {
        if !a.diff.is_empty() {
            self.project.mark_dirty();
        }
    }

    fn remote(&self) -> Result<String, ProjectError> {
        match self.bus.project().setting(lifecycle::REMOTE_SETTING) {
            Some(Value::Text(t)) if !t.trim().is_empty() => Ok(t.trim().to_string()),
            _ => Err(ProjectError::NoRemote),
        }
    }

    /// Link `url` as the remote: an ordinary, undoable edit by the requester, saved with the
    /// next save. Whether it applied.
    fn link_remote(&mut self, issuer: &Issuer, url: &str) -> bool {
        let e = self
            .bus
            .envelope(issuer.clone(), lifecycle::link_remote_command(url));
        let story = e.clone();
        if self.bus.apply(e).is_ok() {
            self.project.mark_dirty();
            self.project.record(story);
            true
        } else {
            false
        }
    }

    /// Perform a lifecycle command the bus accepted (E-36) and record its outcome — or,
    /// for a transfer (push, pull, clone, sign-in, repository creation), start it: the
    /// returned job runs its network part off the core's lock and records the outcome
    /// when it ends.
    fn run_project_op(&mut self, target: &str, args: &str, issuer: &Issuer) -> Option<Job> {
        let who = issuer.tag();
        let mut first_save = false;
        let result = match lifecycle::decode_op(target, args) {
            None => return None,
            // The planner accepted exactly these arguments; a mismatch is still reported.
            Some(Err(why)) => Err(ProjectError::BadFiles(why)),
            Some(Ok(op)) => self.project_op(op, issuer, &who, &mut first_save),
        };
        match result {
            Ok(Step::Done(msg)) => {
                self.project.finish(target, &who, Ok(msg), first_save);
                None
            }
            Ok(Step::Transfer(transfer, what)) => {
                match self.project.begin_transfer(target, &who, what) {
                    Ok(()) => Some(Job {
                        target: target.to_string(),
                        issuer: issuer.clone(),
                        who,
                        transfer,
                    }),
                    Err(e) => {
                        self.project.finish(target, &who, Err(e), false);
                        None
                    }
                }
            }
            Err(e) => {
                self.project.finish(target, &who, Err(e), first_save);
                None
            }
        }
    }

    fn project_op(
        &mut self,
        op: ProjectOp,
        issuer: &Issuer,
        who: &str,
        first_save: &mut bool,
    ) -> Result<Step, ProjectError> {
        match op {
            ProjectOp::Create {
                location,
                name,
                template,
                discard_unsaved,
            } => {
                self.project.check_idle()?;
                if self.project.is_dirty() && !discard_unsaved {
                    return Err(ProjectError::Unsaved);
                }
                let (doc, label) = {
                    let catalog = self.catalog.read().unwrap_or_else(PoisonError::into_inner);
                    let label = match &template {
                        lifecycle::Template::Preset(k) => catalog
                            .get(k)
                            .map_or_else(|| k.clone(), |e| e.preset.workspace.label.clone()),
                        t => t.label().to_string(),
                    };
                    (
                        lifecycle::template_doc_in(&template, &name, &catalog)?,
                        label,
                    )
                };
                self.project.create(&location, who, &doc)?;
                // A project a person creates here is theirs: trusted from the start (WP-34),
                // unless its folder already carried plugins nobody vouched for. What it
                // carries is what the person's template made (WP-35: the answer covers it).
                if issuer.is_human() && crate::trust::carried_plugin_dirs(&location).is_empty() {
                    let key = crate::trust::trust_key(&location);
                    let content = crate::trust::carried_settings(&doc.settings)
                        .into_iter()
                        .map(|i| (i.item, i.digest))
                        .collect();
                    let record =
                        crate::trust::TrustRecord::new(crate::trust::Trust::Trusted, content);
                    if let Err(e) = self.record_trust(&key, record) {
                        self.audit.record(
                            AuditOrigin::Security,
                            "",
                            who,
                            "trust",
                            format!("{key} is trusted for this run only: {e}"),
                        );
                    }
                }
                let held = self.load(&doc, issuer, &location)?;
                Ok(Step::Done(format!(
                    "Created \u{201c}{name}\u{201d} ({label}) at {location}{}",
                    held_note(held)
                )))
            }
            ProjectOp::Open {
                location,
                discard_unsaved,
            } => {
                self.project.check_idle()?;
                if self.project.is_dirty() && !discard_unsaved {
                    return Err(ProjectError::Unsaved);
                }
                let doc = self.project.open_project(&location, who)?;
                let held = self.load(&doc, issuer, &location)?;
                Ok(Step::Done(format!("Opened {location}{}", held_note(held))))
            }
            ProjectOp::Clone {
                remote,
                location,
                discard_unsaved,
            } => {
                self.project.check_idle()?;
                if self.project.is_dirty() && !discard_unsaved {
                    return Err(ProjectError::Unsaved);
                }
                forge_project::host::check_remote(&remote)?;
                let what = format!("Cloning {remote} into {location}");
                Ok(Step::Transfer(
                    Transfer::Clone {
                        remote,
                        location,
                        discard_unsaved,
                        opener: self.project.opener(),
                    },
                    what,
                ))
            }
            ProjectOp::Save { message } => {
                let remote_linked = self.remote().is_ok();
                let bus = &self.bus;
                let keep = |t: TxnId| match bus.txn_state(t) {
                    Some(TxnState::Open) => Keep::Hold,
                    Some(TxnState::Undone | TxnState::Cancelled) => Keep::Drop,
                    // Committed, expired, or retired beyond the tombstone window: it
                    // happened.
                    _ => Keep::Include,
                };
                let s = self
                    .project
                    .save(bus.project(), &message, remote_linked, &keep)?;
                *first_save = s.first;
                Ok(Step::Done(match s.rev {
                    None => "No changes since the last save".to_string(),
                    Some(r) => {
                        let short: String = r.to_string().chars().take(10).collect();
                        format!("Saved revision {short} ({} command(s))", s.commands)
                    }
                }))
            }
            ProjectOp::Push => {
                self.project.check_idle()?;
                let url = self.remote()?;
                self.project.push_check(&url)?;
                let what = format!("Pushing to {url}");
                Ok(Step::Transfer(
                    Transfer::Push {
                        url,
                        opener: self.project.opener(),
                    },
                    what,
                ))
            }
            ProjectOp::Pull => {
                self.project.check_idle()?;
                let url = self.remote()?;
                self.project.pull_check(&url)?;
                let what = format!("Pulling from {url}");
                Ok(Step::Transfer(
                    Transfer::Pull {
                        url,
                        opener: self.project.opener(),
                    },
                    what,
                ))
            }
            ProjectOp::SignIn { resume } => {
                self.project.check_idle()?;
                if resume {
                    let (gh, code) = self.project.sign_in_pending()?;
                    Ok(Step::Transfer(
                        Transfer::SignInContinue { gh, code },
                        "Asking GitHub whether the sign-in was approved".into(),
                    ))
                } else {
                    let gh = self.project.sign_in_client()?;
                    Ok(Step::Transfer(
                        Transfer::SignInStart { gh },
                        "Starting the GitHub sign-in".into(),
                    ))
                }
            }
            ProjectOp::CreateRemote { name } => {
                self.project.check_idle()?;
                let (gh, token) = self.project.github_token()?;
                let what = format!("Creating the GitHub repository {name}");
                Ok(Step::Transfer(
                    Transfer::CreateRemote { gh, token, name },
                    what,
                ))
            }
            ProjectOp::Build { target } => {
                let r = self.project.build(target, self.bus.project())?;
                let unbuilt = r.files.iter().filter(|f| f.unbuilt.is_some()).count();
                Ok(Step::Done(format!(
                    "Built \u{201c}{}\u{201d} for {}: {} file(s){}",
                    r.product.name,
                    target.label(),
                    r.files.len(),
                    if unbuilt > 0 {
                        format!(", {unbuilt} UNBUILT (in-memory packager)")
                    } else {
                        String::new()
                    }
                )))
            }
        }
    }

    /// Wake every client (a transfer ended: its outcome is in the status).
    fn notify_all(&self) {
        for w in self.wakers.values() {
            w();
        }
    }

    /// Wait until every lifecycle transfer started so far has ended (its outcome is then in
    /// the status). Called without the core's lock — the transfers take it to finish — by
    /// a sequential client: the headless script runner, test rigs. The GUI never waits:
    /// its outcome arrives through its waker. Whether there was anything to wait for.
    pub fn wait_transfers(core: &SharedCore) -> bool {
        let mut waited = false;
        loop {
            let running = std::mem::take(&mut lock(core).transfers);
            if running.is_empty() {
                return waited;
            }
            waited = true;
            for t in running {
                // A panicking transfer already recorded its outcome (`run_job`).
                let _ = t.join();
            }
        }
    }

    /// Hand an applied session command to every client that follows them.
    fn deliver(&mut self, cmd: &SessionCommand) {
        for inbox in self.inboxes.values_mut() {
            if inbox.items.len() >= SESSION_INBOX {
                inbox.items.pop_front();
                inbox.dropped += 1;
            }
            inbox.items.push_back(cmd.clone());
        }
    }
}

// ---- lifecycle transfers (WP-16: the network runs off the core's lock) -------------------
//
// A push, pull or clone of a Git remote over HTTPS, and GitHub's device flow and API, take
// as long as the network does (the HTTP client allows 30 s to connect and 60 s per read).
// Holding the core's lock for that would freeze every client — the UI's thread too. So the
// command's checks run under the lock (a refusal is immediate), the status names the
// transfer, and a thread does the network part, taking the lock only for the short local
// steps between (a push's replay into the remote's mirror, a pull's replay into the project,
// loading the result) and to record the outcome, which reaches every client like any other.

/// What a lifecycle operation did under the lock.
enum Step {
    /// Finished: what to tell the user.
    Done(String),
    /// Its network part is still to run (off the lock); what it is doing, for the status.
    Transfer(Transfer, String),
}

/// A transfer's network part and what it needs (opened remotes are the network: opening
/// a Git remote fetches it).
enum Transfer {
    Push {
        url: String,
        opener: StoreOpener,
    },
    Pull {
        url: String,
        opener: StoreOpener,
    },
    Clone {
        remote: String,
        location: String,
        discard_unsaved: bool,
        opener: StoreOpener,
    },
    SignInStart {
        gh: GitHub,
    },
    SignInContinue {
        gh: GitHub,
        code: DeviceCode,
    },
    CreateRemote {
        gh: GitHub,
        token: Credential,
        name: String,
    },
}

/// A started transfer: the command, who asked, and its network part.
struct Job {
    target: String,
    issuer: Issuer,
    who: String,
    transfer: Transfer,
}

/// Run `job` on its own thread (the caller has released the core's lock or is about to:
/// the thread takes it only for its local steps).
fn spawn_job(core: &SharedCore, job: Job) {
    let shared = Arc::clone(core);
    let (target, who) = (job.target.clone(), job.who.clone());
    let started = std::thread::Builder::new()
        .name(format!("forge-transfer:{target}"))
        .spawn(move || run_job(&shared, job));
    let mut c = lock(core);
    match started {
        Ok(handle) => {
            c.transfers.retain(|h| !h.is_finished());
            c.transfers.push(handle);
        }
        Err(e) => {
            c.project.finish_transfer(
                &target,
                &who,
                Err(ProjectError::Interrupted(format!(
                    "its thread could not start: {e}"
                ))),
            );
            c.notify_all();
        }
    }
}

/// The transfer's thread: perform it, then record its outcome and wake every client.
fn run_job(core: &SharedCore, job: Job) {
    let Job {
        target,
        issuer,
        who,
        transfer,
    } = job;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        perform(core, transfer, &issuer, &who)
    }))
    .unwrap_or_else(|payload| {
        let why = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic".into());
        Err(ProjectError::Interrupted(why))
    });
    let mut c = lock(core);
    c.project.finish_transfer(&target, &who, result);
    c.notify_all();
}

/// A transfer's steps: the network without the core's lock, the local steps with it.
fn perform(
    core: &SharedCore,
    transfer: Transfer,
    issuer: &Issuer,
    who: &str,
) -> Result<String, ProjectError> {
    match transfer {
        Transfer::Push { url, opener } => {
            let mut remote = opener(&url, who)?;
            let under_lock = lock(core).faults.push_publishes_under_lock();
            let rep = if under_lock {
                // The W2 control's fault: one guard across the upload.
                let mut c = lock(core);
                let copied = c.project.push_replay(remote.as_mut())?;
                let rep = forge_project::sync::publish(remote.as_mut(), copied)?;
                c.project.pushed(rep.head);
                rep
            } else {
                let copied = lock(core).project.push_replay(remote.as_mut())?;
                let rep = forge_project::sync::publish(remote.as_mut(), copied)?;
                lock(core).project.pushed(rep.head);
                rep
            };
            Ok(if rep.revisions.is_empty() {
                format!("{url} is up to date")
            } else {
                format!("Pushed {} revision(s) to {url}", rep.revisions.len())
            })
        }
        Transfer::Pull { url, opener } => {
            let remote = opener(&url, who)?;
            let mut c = lock(core);
            let (r, doc) = c.project.pull_from(remote.as_ref())?;
            let held = match doc {
                Some(doc) => c.load(&doc, issuer, &url)?,
                None => 0,
            };
            Ok(if r.revisions.is_empty() {
                "Already up to date".to_string()
            } else {
                format!(
                    "Pulled {} revision(s) from {url}{}",
                    r.revisions.len(),
                    held_note(held)
                )
            })
        }
        Transfer::Clone {
            remote,
            location,
            discard_unsaved,
            opener,
        } => {
            let cloned = forge_project::host::clone_into(&opener, &remote, &location, who)?;
            let mut c = lock(core);
            // Edits made while it was cloning are unsaved work too.
            if c.project.is_dirty() && !discard_unsaved {
                return Err(ProjectError::Unsaved);
            }
            let linked = matches!(
                cloned.doc().settings.get(lifecycle::REMOTE_SETTING),
                Some(Value::Text(t)) if t.trim() == remote.trim()
            );
            let doc = c.project.adopt_clone(cloned);
            let held = c.load(&doc, issuer, &remote)?;
            if !linked {
                // The clone stays linked to where it came from.
                c.link_remote(issuer, &remote);
            }
            let revs = c
                .project
                .status()
                .open
                .as_ref()
                .map_or(0, |o| o.revisions.len());
            Ok(format!(
                "Cloned {remote} into {location} ({revs} revision(s)){}",
                held_note(held)
            ))
        }
        Transfer::SignInStart { gh } => {
            let code = gh.start("repo")?;
            Ok(lock(core).project.sign_in_started(gh, code))
        }
        Transfer::SignInContinue { gh, code } => {
            let answer = forge_project::host::sign_in_poll(&gh, &code)?;
            lock(core).project.sign_in_answered(&gh, &code, answer)
        }
        Transfer::CreateRemote { gh, token, name } => {
            let url = gh.create_repo(&token.secret, &name)?.clone_url;
            if !lock(core).link_remote(issuer, &url) {
                return Err(ProjectError::UnsupportedRemote {
                    url,
                    why: "GitHub answered with a URL the project cannot link".into(),
                });
            }
            Ok(format!(
                "Created {url} and linked it as the remote: save, then push"
            ))
        }
    }
}

/// The in-process [`BusClient`] (see the module docs).
pub struct LocalBus {
    core: SharedCore,
    id: u64,
    issuer: Issuer,
    sub: Subscription,
    refused: Vec<Refused>,
    next_ticket: u64,
    /// The project status generation this client last received (`None`: never).
    seen_project: Option<u64>,
}

impl std::fmt::Debug for LocalBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalBus")
            .field("id", &self.id)
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

impl LocalBus {
    /// The core's W2 control switches (read before the core's lock is taken for a request).
    fn faults(&self) -> CoreFaults {
        lock(&self.core).faults
    }

    /// Wake this client's loop when another client changes the project.
    pub fn set_waker(&mut self, w: Waker) {
        let mut c = lock(&self.core);
        c.wake_list
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(self.id, Arc::clone(&w));
        c.wakers.insert(self.id, w);
    }

    /// The core this client talks to (connect a sibling client, e.g. an automation session).
    pub fn core(&self) -> &SharedCore {
        &self.core
    }

    fn ticket(&mut self) -> Ticket {
        let t = Ticket(self.next_ticket);
        self.next_ticket += 1;
        t
    }

    fn run(
        &mut self,
        what: impl FnOnce() -> String,
        f: impl FnOnce(&mut EditorCore, &Issuer) -> Result<(), Rejection>,
    ) -> Ticket {
        let t = self.ticket();
        let mut c = lock(&self.core);
        let r = f(&mut c, &self.issuer);
        // A refusal changed nothing: only a real change wakes the other clients.
        if r.is_ok() {
            c.notify_others(self.id);
        }
        drop(c);
        if let Err(rejection) = r {
            self.refused.push(Refused {
                ticket: t,
                what: what(),
                rejection,
            });
        }
        t
    }

    fn label_of(&self, txn: TxnId) -> String {
        self.txn_info(txn)
            .map(|i| format!("\u{201c}{}\u{201d}", i.label))
            .unwrap_or_else(|| txn.to_string())
    }

    // ---- batches, with results (what automation sessions send) ----------------
    //
    // An automation session needs a request's outcome in the same call (its tool result), and a
    // batch of commands that is all or nothing. These are the same bus operations the `BusClient`
    // requests are, answered synchronously; every event still reaches every client's subscription,
    // and every session command is still delivered.

    /// The effect of `cmds` applied in order (a later one sees what the earlier ones do),
    /// without applying anything. `Err` names the refused command's index.
    pub fn preview_batch(&self, cmds: &[EditorCommand]) -> Result<Vec<Diff>, (usize, Rejection)> {
        let faults = self.faults();
        for (i, cmd) in cmds.iter().enumerate() {
            refuse_direct_load(cmd, &self.issuer, faults).map_err(|r| (i, r))?;
        }
        let mut c = lock(&self.core);
        // One (never opened) transaction id for the whole preview, as the batch would be one
        // transaction: a preview does not use up an id per command.
        let t = c.bus.next_txn_id();
        let envs: Vec<_> = cmds
            .iter()
            .map(|cmd| c.bus.envelope_in(t, self.issuer.clone(), cmd.clone()))
            .collect();
        if c.faults.batch_copies_project() {
            return c.bus.dry_run_batch(&envs);
        }
        // Applied in place and reverted under the lock: no copy of the project (WP-20).
        c.bus.preview_batch(&envs)
    }

    /// Apply `cmds` in order as one batch inside `txn` (or each in its own transaction):
    /// all of them or none. The batch is applied under one lock of the core, so nothing
    /// another client does can land in it; a refusal reverts what it applied before any
    /// client sees it. With `expect_next_key`, the batch is refused (`CMD-0008`, nothing
    /// applied) unless the key allocator is still there (a batch that predicted its spawns'
    /// keys).
    pub fn apply_batch(
        &mut self,
        cmds: Vec<EditorCommand>,
        txn: Option<TxnId>,
        expect_next_key: Option<forge_cmd::EntityKey>,
    ) -> Result<Vec<Arc<forge_cmd::Applied>>, (usize, Rejection)> {
        self.apply_batch_checked(cmds, txn, expect_next_key, &mut |_, _| Ok(()))
    }

    /// [`LocalBus::apply_batch`] with the caller's `check` of each command's diff before it
    /// applies (a batch's `$name` bindings): a refusal by `check` refuses the batch.
    ///
    /// **Cost (WP-20).** One planning pass per command and no copy of the project: the bus
    /// applies each command in place inside the open transaction and reverts the batch on a
    /// refusal ([`forge_cmd::Bus::apply_batch`]). The core's lock is held for what the batch
    /// touches, not for what the project holds, so an automation session's small batch never stalls
    /// the UI on a large project. Security reviews (approve / reject) and collaboration commands in
    /// the batch are checked before anything applies and performed, in order, once the whole batch
    /// applied — as the batch's preview planned it.
    pub fn apply_batch_checked(
        &mut self,
        cmds: Vec<EditorCommand>,
        txn: Option<TxnId>,
        expect_next_key: Option<forge_cmd::EntityKey>,
        check: &mut dyn FnMut(usize, &Diff) -> Result<(), forge_cmd::CmdError>,
    ) -> Result<Vec<Arc<forge_cmd::Applied>>, (usize, Rejection)> {
        let faults = self.faults();
        for (i, cmd) in cmds.iter().enumerate() {
            refuse_direct_load(cmd, &self.issuer, faults).map_err(|r| (i, r))?;
        }
        let mut c = lock(&self.core);
        if let Some(k) = expect_next_key
            && c.bus.project().next_key() != k
        {
            let detail = format!(
                "another client spawned since this batch was planned (next key {}, planned {k})",
                c.bus.project().next_key()
            );
            return Err((
                0,
                Rejection::new(forge_cmd::CmdError::Conflict { detail }, None, txn),
            ));
        }
        // A human may approve or reject automation work in a batch too (checked up front; an
        // automation session is refused by the policy in the dry run).
        let mut reviews = Vec::new();
        if self.issuer.is_human() {
            for (i, cmd) in cmds.iter().enumerate() {
                if let EditorCommand::Invoke { target, args } = cmd
                    && security::REVIEW_TARGETS.contains(&target.as_str())
                {
                    reviews.push((i, target.clone(), c.check_review(args).map_err(|r| (i, r))?));
                }
            }
        }
        // ...and answer held security settings (WP-33), checked up front the same way.
        let mut helds = Vec::new();
        if self.issuer.is_human() {
            for (i, cmd) in cmds.iter().enumerate() {
                if let EditorCommand::Invoke { target, args } = cmd
                    && security::HELD_TARGETS.contains(&target.as_str())
                {
                    helds.push((
                        i,
                        target.clone(),
                        c.check_held(target, args).map_err(|r| (i, r))?,
                    ));
                }
            }
        }
        // Collaboration commands in the batch (WP-U10): checked up front, performed in order.
        for (i, cmd) in cmds.iter().enumerate() {
            if let EditorCommand::Invoke { target, args } = cmd
                && crate::collab::is_target(target)
            {
                c.collab_check(target, args, &self.issuer)
                    .map_err(|r| (i, r))?;
            }
        }
        // What to do once the batch applied: session commands to deliver, collaboration
        // commands to perform (the few `Invoke`s among the batch's commands).
        let performed: Vec<(usize, bool, String, String)> = cmds
            .iter()
            .enumerate()
            .filter_map(|(i, cmd)| match cmd {
                EditorCommand::Invoke { target, args } if c.session_targets.contains(target) => {
                    Some((i, true, target.clone(), args.clone()))
                }
                EditorCommand::Invoke { target, args } if crate::collab::is_target(target) => {
                    Some((i, false, target.clone(), args.clone()))
                }
                _ => None,
            })
            .collect();
        let envs: Vec<_> = cmds
            .into_iter()
            .map(|cmd| match txn {
                Some(t) => c.bus.envelope_in(t, self.issuer.clone(), cmd),
                None => c.bus.envelope(self.issuer.clone(), cmd),
            })
            .collect();
        // Each applied command is the next commit's story (Ch.33 §33.4) while a project is
        // open, and a sandbox delta with a team attached (Ch.37 I19) — as with `apply`.
        let kept = (c.collab.is_some() || c.project.is_open()).then(|| envs.clone());
        let out = if c.faults.batch_copies_project() {
            // The W2 control's fault: the copying preview, then a second planning pass.
            let diffs = c.bus.dry_run_batch(&envs)?;
            for (i, d) in diffs.iter().enumerate() {
                check(i, d).map_err(|e| (i, Rejection::new(e, None, txn)))?;
            }
            let mut out = Vec::with_capacity(envs.len());
            for (i, e) in envs.into_iter().enumerate() {
                out.push(c.bus.apply(e).map_err(|r| (i, r))?);
            }
            out
        } else {
            c.bus.apply_batch(envs, check)?
        };
        let mut jobs = Vec::new();
        let mut failed = None;
        for (i, applied) in out.iter().enumerate() {
            let perf = performed.iter().find(|p| p.0 == i);
            if !applied.diff.is_empty() {
                c.project.mark_dirty();
                if let Some(e) = kept.as_ref().and_then(|k| k.get(i)) {
                    if c.project.is_open() && !perf.is_some_and(|p| p.1) {
                        c.project.record(e.clone());
                    }
                    if c.collab.is_some() {
                        c.collab_record(e);
                    }
                }
            }
            if let Some((_, session, target, args)) = perf {
                if *session && lifecycle::is_session_target(target) {
                    // E-36: the core performs the store operation for every client, an
                    // automation session's batch included; a transfer runs once the lock is
                    // released.
                    let issuer = self.issuer.clone();
                    jobs.extend(c.run_project_op(target, args, &issuer));
                } else if *session {
                    c.deliver(&SessionCommand {
                        seq: applied.seq,
                        issuer: self.issuer.clone(),
                        target: target.clone(),
                        args: args.clone(),
                    });
                } else {
                    let issuer = self.issuer.clone();
                    c.collab_perform(target, args, &issuer);
                }
            }
            if let Some((_, target, id)) = helds.iter().find(|(j, ..)| *j == i) {
                let issuer = self.issuer.clone();
                c.perform_held(target, *id, &issuer);
            }
            if let Some((_, target, (t, auto))) = reviews.iter().find(|(j, ..)| *j == i) {
                let issuer = self.issuer.clone();
                if let Err(r) = c.perform_review(target, *t, auto, &issuer) {
                    // What applied stays applied (and reaches every client); a started
                    // transfer still runs. The review's failure is the answer.
                    failed = Some((i, r));
                    break;
                }
            }
        }
        if !out.is_empty() {
            c.notify_others(self.id);
        }
        drop(c);
        for job in jobs {
            spawn_job(&self.core, job);
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    /// Commit an open transaction, with its result.
    pub fn commit_now(&mut self, txn: TxnId) -> Result<Arc<forge_cmd::Applied>, Rejection> {
        let mut c = lock(&self.core);
        let r = c.bus.commit(txn);
        if r.is_ok() {
            c.notify_others(self.id);
        }
        r
    }

    /// Cancel an open transaction of this issuer, with its result.
    pub fn cancel_now(&mut self, txn: TxnId) -> Result<Arc<forge_cmd::Applied>, Rejection> {
        let mut c = lock(&self.core);
        let r = c.bus.cancel_as(txn, &self.issuer);
        if r.is_ok() {
            c.notify_others(self.id);
        }
        r
    }

    /// Undo a transaction on this issuer's behalf, with its result.
    pub fn undo_now(&mut self, txn: TxnId) -> Result<Arc<forge_cmd::Applied>, Rejection> {
        let mut c = lock(&self.core);
        let r = c.bus.undo_as(txn, &self.issuer);
        if r.is_ok() {
            c.notify_others(self.id);
        }
        r
    }

    /// The policy the core applies to `cmd` (an `Invoke` takes its handler's).
    pub fn policy_of(&self, cmd: &EditorCommand) -> forge_cmd::CommandPolicy {
        lock(&self.core).bus.policy_of(cmd)
    }

    /// The state of a transaction, retired ones included (`None`: never seen).
    pub fn txn_state(&self, txn: TxnId) -> Option<TxnState> {
        lock(&self.core).bus.txn_state(txn)
    }
}

impl Drop for LocalBus {
    fn drop(&mut self) {
        let mut c = lock(&self.core);
        c.wakers.remove(&self.id);
        c.wake_list
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
        c.inboxes.remove(&self.id);
    }
}

/// `forge.project.load` replaces the whole project, its security state included (plugin
/// grants ship with a project). The core performs it when a project is opened, created or
/// pulled; a client that is not a human may not send one itself, or an automation session could
/// load a document of its own holding a grant (Ch.21 §21.18).
fn refuse_direct_load(
    cmd: &EditorCommand,
    issuer: &Issuer,
    faults: CoreFaults,
) -> Result<(), Rejection> {
    match cmd {
        // The team binding and the baseline sync are the core's own (WP-U10): nobody sends
        // them, or a client could bind the project to a team it chose or write a "baseline"
        // of its own, security settings included.
        EditorCommand::Invoke { target, .. }
            if crate::collab::CORE_ONLY.contains(&target.as_str())
                && !faults.core_only_sendable() =>
        {
            Err(Rejection::new(
                forge_cmd::CmdError::PolicyRefused {
                    what: format!(
                        "{target} is performed by the core (creating or joining a team, pulling from its baseline); {issuer} may not send it"
                    ),
                },
                None,
                None,
            ))
        }
        EditorCommand::Invoke { target, .. }
            if target == lifecycle::LOAD_CMD && !issuer.is_human() =>
        {
            Err(Rejection::new(
                forge_cmd::CmdError::PolicyRefused {
                    what: format!(
                        "{target} is performed by the core when a project is opened, created or pulled; {issuer} may not send it"
                    ),
                },
                None,
                None,
            ))
        }
        _ => Ok(()),
    }
}

/// What a lifecycle outcome adds when a non-human's load held security settings back.
fn held_note(held: usize) -> String {
    if held == 0 {
        String::new()
    } else {
        format!(
            "; {held} security setting(s) the project carries (plugin grants, the default automation policy) are held until a person accepts or discards them in the Plugin manager"
        )
    }
}

/// The automation session an issuer is, for the audit (empty for anyone else).
fn issuer_session(issuer: &Issuer) -> String {
    match issuer {
        Issuer::Automation { session, .. } => session.clone(),
        _ => String::new(),
    }
}

/// The session a principal names in the audit (an automation session's session id).
fn session_of(p: &forge_plugin::Principal) -> String {
    match p {
        forge_plugin::Principal::Automation(s) | forge_plugin::Principal::Remote(s) => s.clone(),
        _ => String::new(),
    }
}

fn info(r: &forge_cmd::TxnRecord) -> TxnInfo {
    TxnInfo {
        txn: r.txn(),
        label: r.label().to_string(),
        issuer: r.issuer().clone(),
        state: r.state(),
        opened_at_ms: r.opened_at_ms(),
        undoable: r.undoable(),
    }
}

impl BusClient for LocalBus {
    fn issuer(&self) -> &Issuer {
        &self.issuer
    }

    fn apply(&mut self, cmd: EditorCommand, txn: Option<TxnId>) -> Ticket {
        let what = cmd.label();
        let mut job = None;
        let ticket = self.run(
            || what,
            |c, issuer| {
                refuse_direct_load(&cmd, issuer, c.faults)?;
                // A session command's payload, kept for delivery once the bus accepted it.
                let session = match &cmd {
                    EditorCommand::Invoke { target, args }
                        if c.session_targets.contains(target) =>
                    {
                        Some((target.clone(), args.clone()))
                    }
                    _ => None,
                };
                let links_remote = matches!(
                    &cmd,
                    EditorCommand::Invoke { target, .. } if target == lifecycle::LINK_REMOTE_CMD
                );
                // A human's approval or rejection of automation work: checked now, performed
                // once the bus accepted the command (a non-human one is refused by the bus's
                // policy, CMD-0014, before this matters).
                let review = match &cmd {
                    EditorCommand::Invoke { target, args }
                        if security::REVIEW_TARGETS.contains(&target.as_str())
                            && issuer.is_human() =>
                    {
                        Some((target.clone(), c.check_review(args)?))
                    }
                    _ => None,
                };
                // A human's answer to held security settings (WP-33): checked now, the
                // proposal forgotten once the bus accepted it (an automation session is refused by
                // the policy, CMD-0014).
                let held = match &cmd {
                    EditorCommand::Invoke { target, args }
                        if security::HELD_TARGETS.contains(&target.as_str())
                            && issuer.is_human() =>
                    {
                        Some((target.clone(), c.check_held(target, args)?))
                    }
                    _ => None,
                };
                // A collaboration command (WP-U10): checked against the core's state now,
                // performed on the backends once the bus accepted it.
                let collab = match &cmd {
                    EditorCommand::Invoke { target, args } if crate::collab::is_target(target) => {
                        c.collab_check(target, args, issuer)?;
                        Some((target.clone(), args.clone()))
                    }
                    _ => None,
                };
                let e = match txn {
                    Some(t) => c.bus.envelope_in(t, issuer.clone(), cmd),
                    None => c.bus.envelope(issuer.clone(), cmd),
                };
                // The envelope is the next commit's story (Ch.33 §33.4) once it changed
                // something; kept only while a project is open.
                let story = (session.is_none() && c.project.is_open()).then(|| e.clone());
                // ...and a sandbox delta when the core follows a team baseline (Ch.37 I19).
                let delta = c.collab.is_some().then(|| e.clone());
                let applied = c.bus.apply(e)?;
                if !applied.diff.is_empty() {
                    c.project.mark_dirty();
                    if let Some(e) = story {
                        c.project.record(e);
                    }
                    if let Some(d) = delta {
                        c.collab_record(&d);
                    }
                }
                if let Some((target, args)) = collab {
                    c.collab_perform(&target, &args, issuer);
                }
                if links_remote {
                    c.project.remote_linked();
                }
                if let Some((target, (txn, auto))) = review {
                    c.perform_review(&target, txn, &auto, issuer)?;
                }
                if let Some((target, id)) = held {
                    c.perform_held(&target, id, issuer);
                }
                if let Some((target, args)) = session {
                    if lifecycle::is_session_target(&target) {
                        // E-36: the core performs the store operation; its outcome reaches
                        // every client in the project status. A transfer's network part
                        // runs once the lock is released (below).
                        job = c.run_project_op(&target, &args, issuer);
                    } else {
                        c.deliver(&SessionCommand {
                            seq: applied.seq,
                            issuer: issuer.clone(),
                            target,
                            args,
                        });
                    }
                }
                Ok(())
            },
        );
        if let Some(job) = job {
            spawn_job(&self.core, job);
        }
        ticket
    }

    fn begin(&mut self, label: &str) -> TxnId {
        let issuer = self.issuer.clone();
        lock(&self.core).bus.begin(label, issuer)
    }

    fn commit(&mut self, txn: TxnId) -> Ticket {
        let what = format!("Commit {}", self.label_of(txn));
        self.run(|| what, |c, _| c.bus.commit(txn).map(drop))
    }

    fn cancel(&mut self, txn: TxnId) -> Ticket {
        let what = format!("Cancel {}", self.label_of(txn));
        self.run(
            || what,
            |c, issuer| {
                let a = c.bus.cancel_as(txn, issuer)?;
                c.changed(&a);
                Ok(())
            },
        )
    }

    fn undo(&mut self, txn: TxnId) -> Ticket {
        let what = format!("Undo {}", self.label_of(txn));
        self.run(
            || what,
            |c, issuer| {
                let a = c.bus.undo_as(txn, issuer)?;
                c.changed(&a);
                Ok(())
            },
        )
    }

    fn redo(&mut self, txn: TxnId) -> Ticket {
        let what = format!("Redo {}", self.label_of(txn));
        self.run(
            || what,
            |c, issuer| {
                let a = c.bus.redo_as(txn, issuer)?;
                c.changed(&a);
                Ok(())
            },
        )
    }

    fn project_status(&self) -> Option<Arc<ProjectStatus>> {
        Some(lock(&self.core).project.status())
    }

    fn undo_target(&self) -> Option<TxnId> {
        lock(&self.core).bus.undo_target()
    }

    fn redo_target(&self) -> Option<TxnId> {
        lock(&self.core).bus.redo_target()
    }

    fn preview(&self, cmd: &EditorCommand) -> Result<Diff, Rejection> {
        refuse_direct_load(cmd, &self.issuer, self.faults())?;
        let mut c = lock(&self.core);
        let e = c.bus.envelope(self.issuer.clone(), cmd.clone());
        c.bus.dry_run(&e)
    }

    fn txn_info(&self, txn: TxnId) -> Option<TxnInfo> {
        lock(&self.core).bus.transaction(txn).map(info)
    }

    fn history(&self) -> Vec<TxnInfo> {
        lock(&self.core).bus.history().map(info).collect()
    }

    fn snapshot(&self) -> (Project, u64) {
        let c = lock(&self.core);
        (c.bus.project().clone(), c.bus.next_seq())
    }

    /// A client that only sends session commands (an automation session) need not follow them.
    fn follow_session(&mut self, on: bool) {
        let mut c = lock(&self.core);
        if on {
            c.inboxes.entry(self.id).or_default();
        } else {
            c.inboxes.remove(&self.id);
        }
    }

    fn pump(&mut self) -> Pumped {
        // Under the core's lock, so the session commands and the events are one consistent
        // cut: every session command taken here has its own event in this drain or earlier.
        let mut c = lock(&self.core);
        // Teams (WP-U10): Live pulls the baseline's head here, before this drain, so what a
        // teammate published arrives in this pump like any other change.
        let before = c.bus.next_seq();
        c.collab_follow_inner();
        if c.bus.next_seq() != before {
            c.notify_others(self.id);
        }
        let d = self.sub.drain();
        let (session, session_dropped) = match c.inboxes.get_mut(&self.id) {
            Some(inbox) => (
                inbox.items.drain(..).collect(),
                std::mem::take(&mut inbox.dropped),
            ),
            None => (Vec::new(), 0),
        };
        // The lifecycle status, when it changed since this client last looked (a new
        // client gets it on its first pump).
        let generation = c.project.generation();
        let project = (self.seen_project != Some(generation)).then(|| {
            self.seen_project = Some(generation);
            c.project.status()
        });
        drop(c);
        Pumped {
            gap: d.gap,
            events: d.events,
            refused: std::mem::take(&mut self.refused),
            session,
            session_dropped,
            project,
            // In process, every answer arrives in the turn it was asked: nothing to predict.
            predicted: Vec::new(),
            settled: Vec::new(),
        }
    }
}

/// The split editor's **local prediction** (O-13, Ch.34 §34.2): the editor's own planners
/// over a [`forge_cmd::Replica`] of a remote core's project. A remote client plans each
/// command here — the same dry run the core will run — shows the result at once as a
/// pending overlay, and reconciles when the core answers. It lives in this module because
/// it is the one place allowed to name the bus (the split rule); it owns a planning-only
/// bus that never applies anything, and it cannot reach the core it predicts.
pub struct Predictor {
    planner: Bus,
    replica: forge_cmd::Replica,
    issuer: Issuer,
}

impl std::fmt::Debug for Predictor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Predictor")
            .field("issuer", &self.issuer)
            .field("pending", &self.replica.pending())
            .field("next_seq", &self.replica.next_seq())
            .finish_non_exhaustive()
    }
}

impl Predictor {
    /// A predictor for `issuer`'s commands over `project` as of `next_seq` (a snapshot the
    /// core sent), with every editor handler, the built-in presets' promote planner and the
    /// core's reserved settings — so a command the core would refuse on policy is not
    /// predicted either.
    pub fn new(project: Project, next_seq: u64, issuer: Issuer) -> Self {
        let mut planner = EditorCore::editor_bus();
        for (prefix, writers) in security::RESERVED_SETTINGS {
            planner.reserve_settings(prefix, writers);
        }
        let _ = planner.register_handler(
            lifecycle::PROMOTE_CMD,
            forge_cmd::CommandPolicy::ORDINARY,
            lifecycle::promote::promote_handler(
                lifecycle::promote::builtin_preset_defaults(),
                Arc::default(),
            ),
        );
        Self {
            planner,
            replica: forge_cmd::Replica::new(project, next_seq),
            issuer,
        }
    }

    /// Whether this predictor can plan `cmd` (an `Invoke` of a target only the core has —
    /// a plugin command installed there — cannot be predicted; the client asks the core).
    pub fn knows(&self, cmd: &EditorCommand) -> bool {
        match cmd {
            EditorCommand::Invoke { target, .. } => self.planner.targets().any(|t| t == target),
            _ => true,
        }
    }

    /// The dry run of `cmd` against the predicted project, holding nothing.
    pub fn preview(&self, cmd: &EditorCommand) -> Result<Diff, Rejection> {
        refuse_direct_load(cmd, &self.issuer, CoreFaults::default())?;
        self.planner
            .plan_for(self.replica.project(), &self.issuer, cmd)
            .map_err(|e| Rejection::new(e, None, None))
    }

    /// Predict `cmd` for request `tag`: plan it, apply it on top as a pending prediction,
    /// and return the diff. `None`: nothing to show (it plans to no change, it would be
    /// refused, or its target is unknown here) — the request is sent all the same.
    pub fn predict(&mut self, tag: u64, cmd: &EditorCommand) -> Option<Diff> {
        let diff = self.preview(cmd).ok()?;
        if diff.is_empty() {
            return None;
        }
        self.replica.predict(tag, diff.clone()).ok()?;
        Some(diff)
    }

    /// Follow the core's answers (see [`forge_cmd::Replica::reconcile`]). `Err`: the replica
    /// missed an event and needs a snapshot.
    pub fn reconcile(
        &mut self,
        events: &[Arc<forge_cmd::Applied>],
        settled: &[u64],
    ) -> Result<(), forge_cmd::CmdError> {
        self.replica.reconcile(events, settled)
    }

    /// Replace the replica's authoritative state with a snapshot.
    pub fn resync(&mut self, project: Project, next_seq: u64) {
        self.replica.resync(project, next_seq);
    }

    /// The authoritative project (predictions rolled back).
    pub fn authoritative(&self) -> Project {
        self.replica.authoritative()
    }

    /// The predicted project (what the client shows).
    pub fn predicted(&self) -> &Project {
        self.replica.project()
    }

    /// The `seq` of the next event the replica expects.
    pub fn next_seq(&self) -> u64 {
        self.replica.next_seq()
    }

    /// Predictions not yet settled.
    pub fn pending(&self) -> usize {
        self.replica.pending()
    }

    /// The policy the core applies to `cmd` as far as this predictor knows it (a plugin
    /// command's is the core's to say; an unknown `Invoke` reads as ordinary).
    pub fn policy_of(&self, cmd: &EditorCommand) -> forge_cmd::CommandPolicy {
        self.planner.policy_of(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_cmd::Value;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn human() -> Issuer {
        Issuer::Human { user: "ada".into() }
    }

    #[test]
    fn a_client_sees_every_issuers_changes_and_only_its_own_refusals() {
        let core = EditorCore::new();
        let mut ui = EditorCore::connect(&core, human());
        let mut auto = EditorCore::connect(
            &core,
            Issuer::Automation {
                session: "s1".into(),
                tool: "apply".into(),
            },
        );
        let woke = Arc::new(AtomicU64::new(0));
        let w = woke.clone();
        ui.set_waker(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        auto.apply(
            EditorCommand::Spawn {
                name: "Cube".into(),
                parent: None,
            },
            None,
        );
        assert_eq!(
            woke.load(Ordering::SeqCst),
            1,
            "the session's edit wakes the UI"
        );
        auto.apply(
            EditorCommand::Rename {
                entity: forge_cmd::EntityKey(999),
                name: "x".into(),
            },
            None,
        );
        let p = ui.pump();
        assert_eq!(p.events.len(), 1);
        assert!(
            p.refused.is_empty(),
            "the session's refusal is not the UI's"
        );
        assert_eq!(p.events[0].issuer.tag(), "automation:s1");
        assert_eq!(auto.pump().refused.len(), 1);
        // The UI's own edit does not wake the UI.
        ui.apply(
            EditorCommand::SetSetting {
                key: "a.b".into(),
                value: Some(Value::Int(1)),
            },
            None,
        );
        assert_eq!(woke.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_gesture_is_one_transaction_and_undo_is_attributed_to_the_asker() {
        let core = EditorCore::new();
        let mut ui = EditorCore::connect(&core, human());
        let t = ui.begin("Drag grid");
        for i in 0..5 {
            ui.apply(
                EditorCommand::SetSetting {
                    key: "editor.grid".into(),
                    value: Some(Value::Float(f64::from(i))),
                },
                Some(t),
            );
        }
        ui.commit(t);
        assert_eq!(ui.undo_target(), Some(t));
        assert_eq!(ui.history().len(), 1);
        ui.undo(t);
        assert_eq!(ui.redo_target(), Some(t));
        let p = ui.pump();
        assert!(p.refused.is_empty(), "{:?}", p.refused);
        assert_eq!(p.events.len(), 5 + 1 + 1);
        let (project, next) = ui.snapshot();
        assert!(project.setting("editor.grid").is_none());
        assert_eq!(next, p.events.last().map_or(0, |e| e.seq + 1));
    }
}
