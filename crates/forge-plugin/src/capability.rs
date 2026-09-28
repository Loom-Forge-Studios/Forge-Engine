//! Capabilities — **one grant model** for plugins, automation sessions and remote sessions
//! (E-27, Ch.32.4, Ch.34.5, Ch.37.6): one enum, one grant table, one thing to audit.
//!
//! Nothing is granted by default. A default policy (the project's default automation capability
//! policy, §21.18) may pre-grant only capabilities that are safe to pre-grant: `Net`,
//! `Fs(ProjectWrite)`, `Fs(UserWrite)`, `Process` and `Command(Destructive)` are **never**
//! default-granted. Changing grants is a human-only, audited command (WP-U9); this module is
//! the data model and the check.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::{PluginError, PluginId};

/// File-system scope.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum FsScope {
    /// Read the project's files (through `ProjectStore`).
    ProjectRead,
    /// Write the project's files. Never default-granted.
    ProjectWrite,
    /// Read the user's config directory.
    UserRead,
    /// Write outside the project (the user's files). Never default-granted.
    UserWrite,
}

/// GPU use.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum GpuUse {
    /// Dispatch compute work.
    Compute,
    /// Add render work (passes, draws).
    Render,
}

/// Network use. Never default-granted, in any form.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum NetUse {
    /// Open outbound connections.
    Outbound,
    /// Accept inbound connections.
    Listen,
}

/// Which commands a principal may send to the bus.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum CommandClass {
    /// Ordinary, undoable commands.
    Ordinary,
    /// Destructive commands (delete, overwrite, export, publish; Ch.22.3). Never
    /// default-granted.
    Destructive,
}

/// `Invoke` targets that destroy or publish: they need `Command(Destructive)`. Removing a
/// plugin from the project's plugin set (WP-U9) takes code away from every teammate and
/// build, so it is destructive too.
pub const DESTRUCTIVE_TARGETS: &[&str] = &["forge.asset.remove", "forge.plugin.remove"];

impl CommandClass {
    /// The class a non-human principal needs to send `cmd` (E-27): deleting an entity or a
    /// property, clearing a setting, a destructive `Invoke` target and every command the bus
    /// accepts only from a person (`human_only`, the security class: never undone, so it is
    /// destructive) need `Destructive`; everything else is `Ordinary`. One rule for every
    /// session a host checks against the grant table (remote devices, automation sessions).
    #[must_use]
    pub fn of(cmd: &forge_cmd::EditorCommand, human_only: bool) -> Self {
        use forge_cmd::EditorCommand;
        match cmd {
            EditorCommand::Despawn { .. }
            | EditorCommand::RemoveProperty { .. }
            | EditorCommand::SetSetting { value: None, .. } => Self::Destructive,
            EditorCommand::Invoke { target, .. }
                if human_only || DESTRUCTIVE_TARGETS.contains(&target.as_str()) =>
            {
                Self::Destructive
            }
            _ => Self::Ordinary,
        }
    }
}

/// One capability. The RON spelling is the manifest's: `Fs(ProjectRead)`, `Gpu(Compute)`,
/// `Net(Outbound)`, `Process`, `Command(Destructive)`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Capability {
    /// File-system access.
    Fs(FsScope),
    /// GPU access.
    Gpu(GpuUse),
    /// Network access.
    Net(NetUse),
    /// Spawn processes. Never default-granted.
    Process,
    /// Send commands to the bus.
    Command(CommandClass),
}

impl Capability {
    /// Every capability, for listings and exhaustive tests.
    pub const ALL: [Capability; 11] = [
        Self::Fs(FsScope::ProjectRead),
        Self::Fs(FsScope::ProjectWrite),
        Self::Fs(FsScope::UserRead),
        Self::Fs(FsScope::UserWrite),
        Self::Gpu(GpuUse::Compute),
        Self::Gpu(GpuUse::Render),
        Self::Net(NetUse::Outbound),
        Self::Net(NetUse::Listen),
        Self::Process,
        Self::Command(CommandClass::Ordinary),
        Self::Command(CommandClass::Destructive),
    ];

    /// True if a default policy may pre-grant it (Ch.32.4: `net`, `fs(write)`, `process` and
    /// `command(destructive)` never are).
    #[must_use]
    pub const fn may_be_default(self) -> bool {
        !matches!(
            self,
            Self::Net(_)
                | Self::Fs(FsScope::ProjectWrite | FsScope::UserWrite)
                | Self::Process
                | Self::Command(CommandClass::Destructive)
        )
    }
}

impl Capability {
    /// A stable identifier for the capability (`fs_project_read`, `command_destructive`):
    /// usable as one segment of a project-setting key (the security-class commands store
    /// grants as project settings, Ch.21 §21.18).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Fs(FsScope::ProjectRead) => "fs_project_read",
            Self::Fs(FsScope::ProjectWrite) => "fs_project_write",
            Self::Fs(FsScope::UserRead) => "fs_user_read",
            Self::Fs(FsScope::UserWrite) => "fs_user_write",
            Self::Gpu(GpuUse::Compute) => "gpu_compute",
            Self::Gpu(GpuUse::Render) => "gpu_render",
            Self::Net(NetUse::Outbound) => "net_outbound",
            Self::Net(NetUse::Listen) => "net_listen",
            Self::Process => "process",
            Self::Command(CommandClass::Ordinary) => "command_ordinary",
            Self::Command(CommandClass::Destructive) => "command_destructive",
        }
    }

    /// The capability with this [`Capability::key`].
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }
}

impl std::str::FromStr for Capability {
    type Err = PluginError;

    /// Parse the manifest / display spelling (`Command(Destructive)`, `Process`); spaces are
    /// ignored.
    fn from_str(s: &str) -> Result<Self, PluginError> {
        let want: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        Self::ALL
            .into_iter()
            .find(|c| c.to_string() == want)
            .ok_or_else(|| PluginError::Manifest {
                plugin: None,
                why: format!(
                    "{s:?} is not a capability (e.g. Fs(ProjectRead), Command(Destructive))"
                ),
            })
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fs(s) => write!(f, "Fs({s:?})"),
            Self::Gpu(g) => write!(f, "Gpu({g:?})"),
            Self::Net(n) => write!(f, "Net({n:?})"),
            Self::Process => f.write_str("Process"),
            Self::Command(c) => write!(f, "Command({c:?})"),
        }
    }
}

/// Who holds grants. Plugins, automation sessions, remote sessions and team roles share the one
/// table (E-27).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Principal {
    /// A plugin.
    Plugin(PluginId),
    /// An automation session (a non-human client a plugin hosts over the core).
    Automation(String),
    /// A remote editor session (Ch.34).
    Remote(String),
    /// A team role (Ch.37).
    Role(String),
}

impl fmt::Display for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plugin(p) => write!(f, "plugin:{p}"),
            Self::Automation(s) => write!(f, "automation:{s}"),
            Self::Remote(s) => write!(f, "remote:{s}"),
            Self::Role(r) => write!(f, "role:{r}"),
        }
    }
}

/// The kind of a [`Principal`]: default policies are per kind (the project's default automation
/// capability policy applies to every new automation session).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PrincipalKind {
    /// Plugins.
    Plugin,
    /// Automation sessions.
    Automation,
    /// Remote editor sessions.
    Remote,
    /// Team roles.
    Role,
}

impl Principal {
    /// Its kind.
    #[must_use]
    pub fn kind(&self) -> PrincipalKind {
        match self {
            Self::Plugin(_) => PrincipalKind::Plugin,
            Self::Automation(_) => PrincipalKind::Automation,
            Self::Remote(_) => PrincipalKind::Remote,
            Self::Role(_) => PrincipalKind::Role,
        }
    }
}

/// A default policy: capabilities pre-granted to every new principal of one kind (the
/// project's default automation capability policy). It cannot contain a never-default capability.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefaultPolicy {
    caps: BTreeSet<Capability>,
}

impl DefaultPolicy {
    /// A policy of `caps`; refused (`PLUGIN-0015`) if any of them is never default-granted.
    pub fn new(caps: impl IntoIterator<Item = Capability>) -> Result<Self, PluginError> {
        let caps: BTreeSet<Capability> = caps.into_iter().collect();
        if let Some(bad) = caps.iter().find(|c| !c.may_be_default()) {
            return Err(PluginError::CapabilityDenied {
                principal: "a default policy".into(),
                capability: bad.to_string(),
            });
        }
        Ok(Self { caps })
    }

    /// Its capabilities.
    pub fn capabilities(&self) -> impl Iterator<Item = Capability> + '_ {
        self.caps.iter().copied()
    }

    /// Whether it pre-grants `cap`.
    #[must_use]
    pub fn contains(&self, cap: Capability) -> bool {
        self.caps.contains(&cap)
    }
}

/// The grant table. Empty by default: nothing is granted until a human grants it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    map: BTreeMap<Principal, BTreeSet<Capability>>,
    /// The default policy per principal kind: what a new principal of that kind is
    /// pre-granted by whoever creates it (the host of a new automation session).
    defaults: BTreeMap<PrincipalKind, DefaultPolicy>,
}

impl Grants {
    /// Nothing granted.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Grant `cap` to `who`.
    pub fn grant(&mut self, who: Principal, cap: Capability) {
        self.map.entry(who).or_default().insert(cap);
    }

    /// Pre-grant a default policy to `who` (never-default capabilities cannot be in it).
    pub fn apply_default(&mut self, who: &Principal, policy: &DefaultPolicy) {
        let set = self.map.entry(who.clone()).or_default();
        set.extend(policy.capabilities());
    }

    /// Revoke `cap` from `who`; true if it was granted.
    pub fn revoke(&mut self, who: &Principal, cap: Capability) -> bool {
        self.map.get_mut(who).is_some_and(|s| s.remove(&cap))
    }

    /// Whether `who` holds `cap`.
    #[must_use]
    pub fn has(&self, who: &Principal, cap: Capability) -> bool {
        self.map.get(who).is_some_and(|s| s.contains(&cap))
    }

    /// `Ok` if `who` holds `cap`, else `PLUGIN-0015`.
    pub fn check(&self, who: &Principal, cap: Capability) -> Result<(), PluginError> {
        if self.has(who, cap) {
            Ok(())
        } else {
            Err(PluginError::CapabilityDenied {
                principal: who.to_string(),
                capability: cap.to_string(),
            })
        }
    }

    /// Everything `who` holds.
    pub fn granted(&self, who: &Principal) -> impl Iterator<Item = Capability> + '_ {
        self.map.get(who).into_iter().flatten().copied()
    }

    /// Every principal the table has a row for (a revoke leaves an empty row).
    pub fn principals(&self) -> impl Iterator<Item = &Principal> + '_ {
        self.map.keys()
    }

    /// Set the default policy for new principals of `kind` (a human's decision, made by an
    /// audited command; the caller audits it).
    pub fn set_default_policy(&mut self, kind: PrincipalKind, policy: DefaultPolicy) {
        self.defaults.insert(kind, policy);
    }

    /// The default policy for new principals of `kind`, if one was set.
    #[must_use]
    pub fn default_policy(&self, kind: PrincipalKind) -> Option<&DefaultPolicy> {
        self.defaults.get(&kind)
    }
}

/// The grant table as a live, shared handle: every holder (the WASM plugin host, automation
/// sessions, remote sessions) checks the **same** table at the moment of use, so a grant or a
/// revoke made anywhere takes effect on the next check everywhere (E-27: one thing to audit).
///
/// Checks read-lock; grants and revokes write-lock. A poisoned lock fails closed: every check
/// is denied until the table is replaced.
#[derive(Clone, Debug, Default)]
pub struct SharedGrants(Arc<RwLock<Grants>>);

impl SharedGrants {
    /// An empty shared table (nothing granted).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Share `grants`.
    #[must_use]
    pub fn from_grants(grants: Grants) -> Self {
        Self(Arc::new(RwLock::new(grants)))
    }

    /// Whether `who` holds `cap` right now (false on a poisoned table).
    #[must_use]
    pub fn has(&self, who: &Principal, cap: Capability) -> bool {
        self.0.read().is_ok_and(|g| g.has(who, cap))
    }

    /// `Ok` if `who` holds `cap` right now, else `PLUGIN-0015`.
    pub fn check(&self, who: &Principal, cap: Capability) -> Result<(), PluginError> {
        if self.has(who, cap) {
            Ok(())
        } else {
            Err(PluginError::CapabilityDenied {
                principal: who.to_string(),
                capability: cap.to_string(),
            })
        }
    }

    /// Grant `cap` to `who` (a human's decision; the caller audits it).
    pub fn grant(&self, who: Principal, cap: Capability) {
        if let Ok(mut g) = self.0.write() {
            g.grant(who, cap);
        }
    }

    /// Revoke `cap` from `who`; true if it was granted.
    pub fn revoke(&self, who: &Principal, cap: Capability) -> bool {
        self.0.write().is_ok_and(|mut g| g.revoke(who, cap))
    }

    /// Pre-grant a default policy to `who`.
    pub fn apply_default(&self, who: &Principal, policy: &DefaultPolicy) {
        if let Ok(mut g) = self.0.write() {
            g.apply_default(who, policy);
        }
    }

    /// A copy of the table as it is now (empty on a poisoned table).
    #[must_use]
    pub fn snapshot(&self) -> Grants {
        self.0.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Set the default policy for new principals of `kind` (see [`Grants::set_default_policy`]).
    pub fn set_default_policy(&self, kind: PrincipalKind, policy: DefaultPolicy) {
        if let Ok(mut g) = self.0.write() {
            g.set_default_policy(kind, policy);
        }
    }

    /// The default policy for new principals of `kind`, if one was set (`None` on a
    /// poisoned table).
    #[must_use]
    pub fn default_policy(&self, kind: PrincipalKind) -> Option<DefaultPolicy> {
        self.0
            .read()
            .ok()
            .and_then(|g| g.default_policy(kind).cloned())
    }

    /// Everything `who` holds right now (nothing on a poisoned table).
    #[must_use]
    pub fn granted(&self, who: &Principal) -> Vec<Capability> {
        self.0
            .read()
            .map(|g| g.granted(who).collect())
            .unwrap_or_default()
    }

    /// True if both handles share one table.
    #[must_use]
    pub fn same_table(&self, other: &SharedGrants) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_granted_by_default_and_never_default_caps_stay_out_of_policies() {
        let g = Grants::new();
        let p = Principal::Plugin(PluginId::new("com.example.rivers").expect("id"));
        for c in Capability::ALL {
            assert!(!g.has(&p, c), "{c} granted by default");
        }
        let never: Vec<Capability> = Capability::ALL
            .into_iter()
            .filter(|c| !c.may_be_default())
            .collect();
        assert_eq!(
            never,
            [
                Capability::Fs(FsScope::ProjectWrite),
                Capability::Fs(FsScope::UserWrite),
                Capability::Net(NetUse::Outbound),
                Capability::Net(NetUse::Listen),
                Capability::Process,
                Capability::Command(CommandClass::Destructive),
            ]
        );
        for c in never {
            assert_eq!(
                DefaultPolicy::new([c]).map_err(|e| e.code().as_str()),
                Err("PLUGIN-0015"),
                "{c}"
            );
        }
    }

    #[test]
    fn grants_are_per_principal_and_revocable() {
        let mut g = Grants::new();
        let auto = Principal::Automation("s1".into());
        let other = Principal::Automation("s2".into());
        let policy = DefaultPolicy::new([
            Capability::Fs(FsScope::ProjectRead),
            Capability::Command(CommandClass::Ordinary),
        ])
        .expect("safe policy");
        g.apply_default(&auto, &policy);
        g.grant(auto.clone(), Capability::Command(CommandClass::Destructive));
        assert!(
            g.check(&auto, Capability::Command(CommandClass::Destructive))
                .is_ok()
        );
        let e = g
            .check(&other, Capability::Command(CommandClass::Ordinary))
            .expect_err("not granted");
        assert!(
            e.to_string().contains("automation:s2") && e.to_string().contains("Command(Ordinary)")
        );
        assert!(g.revoke(&auto, Capability::Command(CommandClass::Destructive)));
        assert!(!g.has(&auto, Capability::Command(CommandClass::Destructive)));
        assert_eq!(g.granted(&auto).count(), 2);
    }

    #[test]
    fn a_shared_table_sees_every_holders_grants_and_revokes_at_once() {
        let a = SharedGrants::new();
        let b = a.clone();
        let p = Principal::Plugin(PluginId::new("com.example.rivers").expect("id"));
        let cap = Capability::Fs(FsScope::ProjectRead);
        assert!(b.check(&p, cap).is_err());
        a.grant(p.clone(), cap);
        assert!(
            b.check(&p, cap).is_ok(),
            "a grant through one handle is seen by the other"
        );
        assert!(b.revoke(&p, cap));
        assert!(
            !a.has(&p, cap),
            "a revoke through one handle is seen by the other"
        );
        assert!(a.same_table(&b) && !a.same_table(&SharedGrants::new()));
        assert_eq!(a.snapshot(), Grants::new().clone_with(&p));
    }

    impl Grants {
        /// An empty table that still has `who`'s (now empty) row, as a revoke leaves it.
        fn clone_with(mut self, who: &Principal) -> Self {
            self.map.entry(who.clone()).or_default();
            self
        }
    }

    #[test]
    fn keys_and_spellings_round_trip_and_default_policies_are_per_kind() {
        for c in Capability::ALL {
            assert_eq!(Capability::from_key(c.key()), Some(c));
            assert_eq!(c.to_string().parse::<Capability>(), Ok(c));
            assert!(key_segment(c.key()), "{c}");
        }
        assert!("Command (Destructive)".parse::<Capability>().is_ok());
        assert!("Everything".parse::<Capability>().is_err());
        let g = SharedGrants::new();
        assert_eq!(g.default_policy(PrincipalKind::Automation), None);
        let p = DefaultPolicy::new([Capability::Fs(FsScope::ProjectRead)]).expect("safe");
        assert!(p.contains(Capability::Fs(FsScope::ProjectRead)));
        g.set_default_policy(PrincipalKind::Automation, p.clone());
        assert_eq!(g.default_policy(PrincipalKind::Automation), Some(p));
        assert_eq!(g.default_policy(PrincipalKind::Remote), None);
        assert_eq!(
            Principal::Automation("a".into()).kind(),
            PrincipalKind::Automation
        );
    }

    /// A key is one `[a-z_][a-z0-9_]*` segment (a project-setting path segment).
    fn key_segment(k: &str) -> bool {
        let mut c = k.chars();
        c.next().is_some_and(|f| f.is_ascii_lowercase() || f == '_')
            && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_')
    }

    #[test]
    fn capabilities_use_the_manifest_spelling_in_ron() {
        let caps: Vec<Capability> =
            ron::from_str("[Fs(ProjectRead), Gpu(Compute), Process, Command(Destructive)]")
                .expect("parses");
        assert_eq!(caps[0].to_string(), "Fs(ProjectRead)");
        assert_eq!(caps[2], Capability::Process);
        let text = ron::to_string(&Capability::Net(NetUse::Outbound)).expect("writes");
        assert_eq!(text, "Net(Outbound)");
    }
}
