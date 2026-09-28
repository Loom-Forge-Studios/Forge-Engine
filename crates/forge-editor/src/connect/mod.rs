//! The read-side services of the **connect** panels (Ch.21 §21.21, WP-U9): the plugin
//! manager, the audit log, remote connect, and compute & farm.
//!
//! | Service | Backend | Real or D-4 |
//! |---|---|---|
//! | [`plugins::PluginManager`] | the ordinary loader's manifests, `forge-wasm` hosted plugins, the project's grants | real |
//! | [`plugins::PluginIndex`] | [`plugins::MemoryIndex`] | D-4 until the signed index (M5-14) |
//! | [`plugins::PluginCache`] | user config (`<config>/plugins`) | real |
//! | [`audit::AuditSource`] | [`audit::EditorAudit`]: the core's book, plus the [`audit::AuditFeed`] a plugin attaches | real; teammates D-4 (`forge-collab`) |
//! | [`remote::RemoteTransport`] | [`remote::LoopbackTransport`] | D-4 until `forge-remote` (M2-16) |
//! | [`remote::RemotePairingStore`] | user config (`<config>/remote_pairing.ron`) | real |
//! | [`compute::ComputePool`] | [`compute::MemoryComputePool`] | D-4 until `forge-jobs` / `forge-farm` (M6-1, M6-3) |
//!
//! None of them changes project state: every such change the panels make is a command
//! (security commands, plugin set changes, approvals). The pairing store and the plugin
//! cache write user config (§21.18's allow-listed writers), through the editor's one
//! atomic user-config writer.
//!
//! A plugin that hosts its own sessions over the core (a protocol server) adds its services
//! beside these through the editor services' typed extension slot
//! ([`crate::services::ServiceExtensions`]) and its log through
//! [`ConnectServices::attach_audit_feed`]; the editor names none of them.

pub mod audit;
pub mod compute;
pub mod plugins;
pub mod remote;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use crate::core::{EditorCore, SharedCore};

/// What a capability lets its holder do, as a person reads it in the UI locale (the
/// manifest spelling, `Fs(ProjectRead)`, is `Capability`'s `Display`, for files and logs).
#[must_use]
pub fn capability_label(c: forge_plugin::Capability) -> &'static str {
    use forge_plugin::{Capability, CommandClass, FsScope, GpuUse, NetUse};
    match c {
        Capability::Fs(FsScope::ProjectRead) => forge_ui::tr!("read the project's files"),
        Capability::Fs(FsScope::ProjectWrite) => forge_ui::tr!("write the project's files"),
        Capability::Fs(FsScope::UserRead) => forge_ui::tr!("read your user settings"),
        Capability::Fs(FsScope::UserWrite) => {
            forge_ui::tr!("write files outside the project (your files)")
        }
        Capability::Gpu(GpuUse::Compute) => forge_ui::tr!("run compute work on the GPU"),
        Capability::Gpu(GpuUse::Render) => forge_ui::tr!("add render work on the GPU"),
        Capability::Net(NetUse::Outbound) => forge_ui::tr!("connect out to the network"),
        Capability::Net(NetUse::Listen) => forge_ui::tr!("accept network connections"),
        Capability::Process => forge_ui::tr!("start other programs"),
        Capability::Command(CommandClass::Ordinary) => forge_ui::tr!("send undoable commands"),
        Capability::Command(CommandClass::Destructive) => {
            forge_ui::tr!("send destructive commands (delete, overwrite, export, publish)")
        }
    }
}

/// The connect panels' services (see the module docs).
pub struct ConnectServices {
    pub plugins: Rc<RefCell<plugins::PluginManager>>,
    pub index: Rc<dyn plugins::PluginIndex>,
    pub cache: Rc<RefCell<plugins::PluginCache>>,
    pub audit: Rc<dyn audit::AuditSource>,
    /// The book the audit panel reads and the pairing store writes (the core's, when
    /// connected with [`ConnectServices::for_core`]).
    pub book: audit::AuditBook,
    pub remote: Rc<RefCell<dyn remote::RemoteTransport>>,
    pub pairing: Rc<RefCell<remote::RemotePairingStore>>,
    pub compute: Arc<dyn compute::ComputePool>,
    /// A live cell bumped whenever the audit book changes (the audit panel's feed; an
    /// attached [`audit::AuditFeed`] has its own, [`audit::AuditSource::live`]).
    pub audit_live: Arc<forge_ui::LiveCell>,
    /// Whether these services follow a core (`false`: the defaults, which say so).
    pub connected: bool,
    /// The core they follow, if any: the Plugin manager reads and answers the open project's
    /// trust through it (WP-34, [`crate::trust`]), and the security settings a non-human's
    /// load held back (WP-33).
    pub core: Option<SharedCore>,
    /// Why the pairing file could not be read (the store started empty).
    pub pairing_error: Option<String>,
    #[doc(hidden)]
    pub faults: ConnectFaults,
}

forge_trace::control_switches! {
    /// W2 positive-control switches for the connect panels' guards. Never set outside them.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct ConnectFaults {
        /// The latency readout refreshes at 60 Hz instead of at most 2.
        pub latency_uncapped: bool,
        /// The compute panel refreshes at 60 Hz instead of at most 2.
        pub compute_uncapped: bool,
        /// Approve / reject are sent as an ordinary setting write instead of the security
        /// command: the transaction stays open and nothing is audited (the review panel's
        /// guard's control).
        pub review_not_a_command: bool,
        /// A panel polls its backend every loop turn instead of following its live feed
        /// (`test_connect_panels_idle`'s control: an idle editor that never sleeps).
        pub poll_backends: bool,
        /// The audit panel re-reads and rebuilds its whole list on every refresh instead of
        /// appending what is new (`test_audit_log_panel`'s follow-cost control).
        pub audit_full_rebuild: bool,
    }
}

impl std::fmt::Debug for ConnectServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectServices")
            .field("connected", &self.connected)
            .field("audit", &self.audit.backend())
            .finish_non_exhaustive()
    }
}

impl Default for ConnectServices {
    /// Not connected to a core: the manager lists nothing, an empty book.
    fn default() -> Self {
        let book = audit::AuditBook::default();
        Self::assemble(
            book.clone(),
            Rc::new(audit::EditorAudit {
                book,
                feed: None,
                skip_security: false,
            }),
            None,
            false,
        )
    }
}

impl ConnectServices {
    fn assemble(
        book: audit::AuditBook,
        audit: Rc<dyn audit::AuditSource>,
        config_dir: Option<&Path>,
        connected: bool,
    ) -> Self {
        let audit_live = forge_ui::LiveCell::new();
        audit_live.set_live(true);
        let cell = Arc::clone(&audit_live);
        book.set_observer(Some(Arc::new(move || cell.bump())));
        let (pairing, err) = remote::RemotePairingStore::open(config_dir, book.clone());
        Self {
            plugins: Rc::new(RefCell::new(plugins::PluginManager::default())),
            index: Rc::new(plugins::MemoryIndex::sample()),
            cache: Rc::new(RefCell::new(plugins::PluginCache::new(config_dir))),
            audit,
            book,
            remote: Rc::new(RefCell::new(remote::LoopbackTransport::default())),
            pairing: Rc::new(RefCell::new(pairing)),
            compute: Arc::new(compute::MemoryComputePool::new()),
            audit_live,
            connected,
            core: None,
            pairing_error: err.map(|e| e.to_string()),
            faults: ConnectFaults::default(),
        }
    }

    /// Services following `core`: its audit book and its security state. `config_dir` holds
    /// the plugin cache and the pairing file (`None`: memory only).
    pub fn for_core(core: &SharedCore, config_dir: Option<&Path>) -> Self {
        let book = EditorCore::audit_book(core);
        let audit_src = Rc::new(audit::EditorAudit {
            book: book.clone(),
            feed: None,
            skip_security: false,
        });
        let mut s = Self::assemble(book, audit_src, config_dir, true);
        s.core = Some(core.clone());
        s
    }

    /// Follow `core`, keeping the plugin manager, the index, the cache, the transport and
    /// the compute pool: what [`ConnectServices::for_core`] does to services `assemble`
    /// already filled.
    pub fn attach(&mut self, core: &SharedCore, config_dir: Option<&Path>) {
        let fresh = Self::for_core(core, config_dir);
        self.audit = fresh.audit;
        self.book = fresh.book;
        self.pairing = fresh.pairing;
        self.pairing_error = fresh.pairing_error;
        self.audit_live = fresh.audit_live;
        self.cache = fresh.cache;
        self.core = fresh.core;
        self.connected = true;
    }

    /// Join `feed` to the audit timeline (a plugin's own log; see [`audit::AuditFeed`]):
    /// the audit panel then reads the book and the feed as one, and follows the feed's live
    /// source. Replaces a feed attached before.
    pub fn attach_audit_feed(&mut self, feed: Arc<dyn audit::AuditFeed>) {
        self.audit = Rc::new(audit::EditorAudit {
            book: self.book.clone(),
            feed: Some(feed),
            skip_security: false,
        });
    }

    /// The same services with a plugin manager built from a load's manifests.
    #[must_use]
    pub fn with_plugins(self, manager: plugins::PluginManager) -> Self {
        *self.plugins.borrow_mut() = manager;
        self
    }

    /// Security settings a non-human's project load held back for a person to accept or
    /// discard (WP-33; `forge.security.accept_held` / `discard_held`), if these services
    /// follow a core.
    #[must_use]
    pub fn held_security(&self) -> Option<crate::security::HeldSecurity> {
        self.core.as_ref().and_then(EditorCore::held_security)
    }

    /// The held proposal's id: what a panel follows each turn without cloning it.
    #[must_use]
    pub fn held_security_id(&self) -> Option<u64> {
        self.core.as_ref().and_then(EditorCore::held_security_id)
    }
}
