//! **Teams, sandboxes and the team baseline** — the server side of Ch.37, as the traits the
//! editor core reaches it through, with **labelled in-memory implementations (D-4)** until
//! `forge-identity` (M5-15) and `forge-collab` / `forge-server` (M5-18, M6-10) exist
//! (WP-U10, ADR 0039).
//!
//! | Trait | What it is | In-memory stand-in |
//! |---|---|---|
//! | [`IdentityView`] / [`IdentityBackend`] | accounts, teams, members, roles, per-path scopes, pending invites (email or join code) — the identity database Ch.37 §37.8 puts on the server | [`MemoryIdentity`] |
//! | [`CollabView`] / [`CollabBackend`] | the team **baseline** (a real [`ProjectStore`]: memory, a `LocalFs` folder or a **Git** repository), the sandbox rows (I19: a sandbox is a row, not a process), Live/Pull policies (I20), presence, scoped ownership claims, the publish/review queue | [`MemoryCollab`] |
//!
//! **Each pair is split in two on purpose.** The `*View` half reads, and writes only
//! *session* state (presence, a person's own Live/Pull policy and sandbox visibility — none of
//! it project state, Ch.21 §21.18); panels get it. The `*Backend` half changes the team or
//! the baseline, and **only the editor core names it** (the I7 guard lists both as
//! project-state writers): a panel publishes, claims or invites by sending a command, which
//! the core checks against the issuer's role *per command* and then performs (E-36's shape).
//!
//! **Publishing is `ProjectStore::commit`** (Ch.37 §37.3): the sandbox's document as the
//! working files, committed with the envelopes that made it, on a baseline whose head must
//! still be the sandbox's base (a fast-forward; otherwise the sandbox pulls first). So the
//! baseline's history is the store's history — on a Git baseline, Git commits carrying the
//! command log.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use forge_cmd::CommandEnvelope;
use forge_store::{Blake3, Bytes, ProjectStore, RevId, RevRange, StorePath};
use serde::{Deserialize, Serialize};

use crate::ProjectError;
use crate::format::{MANIFEST_PATH, ProjectDoc, SCENE_PATH, SETTINGS_PATH};

/// How the in-memory stand-ins name themselves (panels and `just gate` show it).
pub const IN_MEMORY: &str = "in-memory stand-in (D-4)";

/// Called after a backend changed (never under the backend's lock).
pub type Observer = Arc<dyn Fn() + Send + Sync>;

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every method leaves the maps whole before it returns; a poisoned lock is consistent.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct Observers {
    next: AtomicU64,
    list: Mutex<BTreeMap<u64, Observer>>,
    generation: AtomicU64,
}

impl Observers {
    fn add(&self, f: Observer) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        guard(&self.list).insert(id, f);
        id
    }
    fn remove(&self, id: u64) {
        guard(&self.list).remove(&id);
    }
    /// Bump the generation and tell every observer (called with no backend lock held).
    fn changed(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let list: Vec<Observer> = guard(&self.list).values().cloned().collect();
        for f in list {
            f();
        }
    }
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

// ---- roles ---------------------------------------------------------------------------------

/// A team role (Ch.37 §37.6): a capability set on the one grant model, so there is one thing
/// to audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Role {
    /// Everything, including removing members and making Owners.
    Owner,
    /// Publish to the baseline, manage members below Maintainer, manage claims, approve.
    Maintainer,
    /// Own a sandbox, claim scopes, publish (optionally review-gated, per path).
    Developer,
    /// Read the baseline and shared sandboxes, approve publish requests.
    Reviewer,
    /// Read the baseline only.
    Viewer,
}

impl Role {
    /// Every role, highest first.
    pub const ALL: [Role; 5] = [
        Self::Owner,
        Self::Maintainer,
        Self::Developer,
        Self::Reviewer,
        Self::Viewer,
    ];
    /// Its name in commands (`developer`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Maintainer => "maintainer",
            Self::Developer => "developer",
            Self::Reviewer => "reviewer",
            Self::Viewer => "viewer",
        }
    }
    /// What a person reads (`Developer`).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::Maintainer => "Maintainer",
            Self::Developer => "Developer",
            Self::Reviewer => "Reviewer",
            Self::Viewer => "Viewer",
        }
    }
    /// Parse [`Role::name`] (case-insensitive).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|r| r.name().eq_ignore_ascii_case(s.trim()))
    }
    /// Change the sandbox (edit), claim, publish.
    #[must_use]
    pub const fn edits(self) -> bool {
        matches!(self, Self::Owner | Self::Maintainer | Self::Developer)
    }
    /// Invite, remove, change roles and scopes (below its own rank, for a Maintainer).
    #[must_use]
    pub const fn manages_members(self) -> bool {
        matches!(self, Self::Owner | Self::Maintainer)
    }
    /// Approve or reject a publish request.
    #[must_use]
    pub const fn approves(self) -> bool {
        matches!(self, Self::Owner | Self::Maintainer | Self::Reviewer)
    }
    /// Publish without review, release anyone's claim, set the review rules.
    #[must_use]
    pub const fn maintains(self) -> bool {
        matches!(self, Self::Owner | Self::Maintainer)
    }
    /// Read a teammate's `Private` sandbox (Ch.37 §37.8: only its owner and the Owner role).
    #[must_use]
    pub const fn reads_private(self) -> bool {
        matches!(self, Self::Owner)
    }
}

/// May `actor` give a member whose role is `current` (`None`: an invitee) the role `new`?
/// Owners may do anything; a Maintainer only below Maintainer, and never to someone at or
/// above it. **Nobody raises a role above their own** (the escalation `test_role_enforcement`
/// tries). `Err` says why, for the refusal.
pub fn may_assign(actor: Option<Role>, current: Option<Role>, new: Role) -> Result<(), String> {
    let Some(actor) = actor else {
        return Err("you are not a member of this team".into());
    };
    if !actor.manages_members() {
        return Err(format!(
            "a {} cannot manage members (Owners and Maintainers can)",
            actor.label()
        ));
    }
    if actor == Role::Owner {
        return Ok(());
    }
    if new <= Role::Maintainer {
        return Err(format!(
            "a Maintainer cannot make anyone a {}: only an Owner can",
            new.label()
        ));
    }
    if current.is_some_and(|c| c <= Role::Maintainer) {
        return Err(format!(
            "a Maintainer cannot change a {}'s membership: only an Owner can",
            current.map_or("", Role::label)
        ));
    }
    Ok(())
}

// ---- identity -----------------------------------------------------------------------------

/// A local account (Ch.37 §37.6: local accounts on the self-hosted server are the default;
/// bring-your-own OIDC maps onto the same record).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub user: String,
    pub display: String,
    pub email: String,
}

/// A team member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub user: String,
    pub role: Role,
    /// Publish scopes (globs over content paths, `scene/Levels/**`); empty: everywhere.
    pub paths: Vec<String>,
    pub since_ms: u64,
}

/// Who an invite is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InviteTo {
    /// The account with this email accepts it.
    Email(String),
    /// Whoever has this join code (`ABCD-EFGH`).
    Code(String),
}

/// A pending invite: visible and revocable until it is accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    pub id: String,
    pub to: InviteTo,
    pub role: Role,
    pub paths: Vec<String>,
    pub by: String,
    pub at_ms: u64,
}

impl Invite {
    /// `bob@studio.example` or `join code ABCD-EFGH` (`a join code` when it is withheld).
    #[must_use]
    pub fn target(&self) -> String {
        match &self.to {
            InviteTo::Email(e) => e.clone(),
            InviteTo::Code(c) if c.is_empty() => "a join code".to_string(),
            InviteTo::Code(c) => format!("join code {c}"),
        }
    }
    /// A join code whose text this reader may not see ([`IdentityView::team`]).
    #[must_use]
    pub fn code_withheld(&self) -> bool {
        matches!(&self.to, InviteTo::Code(c) if c.is_empty())
    }
}

/// A team.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub members: Vec<Member>,
    pub invites: Vec<Invite>,
}

impl Team {
    #[must_use]
    pub fn member(&self, user: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.user == user)
    }
    #[must_use]
    pub fn role_of(&self, user: &str) -> Option<Role> {
        self.member(user).map(|m| m.role)
    }
    fn owners(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.role == Role::Owner)
            .count()
    }
    /// The team with every join code's text withheld (the invite stays listed, with its
    /// role; [`Invite::code_withheld`]).
    #[must_use]
    pub fn withholding_codes(self) -> Self {
        self.withholding_codes_from(None)
    }
    /// The team as `viewer` may read it: a join code's text only where `viewer` could have
    /// minted that invite ([`may_assign`] for its role: an Owner every code, a Maintainer the
    /// codes below Maintainer); every other code is withheld. A code is a bearer credential
    /// for its role, so reading one is as good as being given that role.
    #[must_use]
    pub fn withholding_codes_from(mut self, viewer: Option<&str>) -> Self {
        let role = viewer.and_then(|v| self.role_of(v));
        for i in &mut self.invites {
            if let InviteTo::Code(c) = &mut i.to
                && may_assign(role, None, i.role).is_err()
            {
                c.clear();
            }
        }
        self
    }
}

/// What an invite is made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InviteSpec {
    /// `Some`: for that account's email; `None`: a join code is minted.
    pub email: Option<String>,
    pub role: Role,
    pub paths: Vec<String>,
}

/// The identity database, read side (panels get this).
pub trait IdentityView: Send + Sync {
    /// What serves it (`in-memory stand-in (D-4)`).
    fn backend(&self) -> &'static str;
    /// Bumped on every change.
    fn generation(&self) -> u64;
    fn account(&self, user: &str) -> Option<Account>;
    /// The team with its **join codes withheld**: a code is a bearer credential for its role
    /// (whoever types it joins as that role), so the read side never carries one except to
    /// someone who could mint it ([`IdentityView::team_as`]).
    fn team(&self, id: &str) -> Option<Team>;
    /// The team as `viewer` sees it: a join code's text only where `viewer` could have minted
    /// that invite ([`Team::withholding_codes_from`]); withheld for everyone else.
    fn team_as(&self, id: &str, viewer: &str) -> Option<Team>;
    /// Email invites waiting for `user`'s account: `(team id, team name, invite)`.
    fn invites_for(&self, user: &str) -> Vec<(String, String, Invite)>;
    fn observe(&self, f: Observer) -> u64;
    fn unobserve(&self, id: u64);
}

/// The identity database, write side: **only the editor core names it** (after checking the
/// issuer's role on the command, per command).
pub trait IdentityBackend: IdentityView {
    /// The whole record, join codes included (the core checks a code against it).
    fn team_record(&self, id: &str) -> Option<Team>;
    /// Make sure `user` has a local account (signing in on this machine).
    fn ensure_account(&self, user: &str, email: Option<&str>);
    /// A new team with `owner` as its only Owner.
    fn create_team(&self, name: &str, owner: &str, now_ms: u64) -> Result<Team, ProjectError>;
    /// A pending invite (a join code is minted when it is not for an email).
    fn invite(
        &self,
        team: &str,
        spec: InviteSpec,
        by: &str,
        now_ms: u64,
    ) -> Result<Invite, ProjectError>;
    fn revoke_invite(&self, team: &str, invite: &str) -> Result<Invite, ProjectError>;
    /// `user` accepts with a join code, or the id of an email invite for their account.
    fn accept(&self, user: &str, token: &str, now_ms: u64) -> Result<(Team, Member), ProjectError>;
    /// The member's previous role. Never leaves a team without an Owner.
    fn set_role(&self, team: &str, user: &str, role: Role) -> Result<Role, ProjectError>;
    fn set_paths(&self, team: &str, user: &str, paths: Vec<String>) -> Result<(), ProjectError>;
    /// Never removes the last Owner.
    fn remove_member(&self, team: &str, user: &str) -> Result<Member, ProjectError>;
}

/// The in-memory identity database (D-4, labelled): local accounts only, lives as long as
/// the process. `forge-identity` (M5-15) replaces it behind the same traits.
#[derive(Default)]
pub struct MemoryIdentity {
    state: Mutex<IdState>,
    observers: Observers,
}

#[derive(Default)]
struct IdState {
    accounts: BTreeMap<String, Account>,
    teams: BTreeMap<String, Team>,
    next: u64,
}

fn team_err(why: impl Into<String>) -> ProjectError {
    ProjectError::Team(why.into())
}

/// A join code: eight characters from an alphabet without look-alikes, `ABCD-EFGH`.
fn mint_code(seed: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let h = Blake3::of(seed);
    let mut s = String::with_capacity(9);
    for (i, b) in h.0.iter().take(8).enumerate() {
        if i == 4 {
            s.push('-');
        }
        s.push(char::from(ALPHABET[usize::from(*b) % ALPHABET.len()]));
    }
    s
}

impl MemoryIdentity {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// With an account (tests and the editor's local sign-in).
    #[must_use]
    pub fn with_account(self, user: &str, email: &str) -> Self {
        self.ensure_account(user, Some(email));
        self
    }

    fn edit<R>(
        &self,
        f: impl FnOnce(&mut IdState) -> Result<R, ProjectError>,
    ) -> Result<R, ProjectError> {
        let r = f(&mut guard(&self.state));
        if r.is_ok() {
            self.observers.changed();
        }
        r
    }
}

fn team_mut<'a>(s: &'a mut IdState, team: &str) -> Result<&'a mut Team, ProjectError> {
    s.teams
        .get_mut(team)
        .ok_or_else(|| team_err(format!("there is no team {team}")))
}

impl IdentityView for MemoryIdentity {
    fn backend(&self) -> &'static str {
        IN_MEMORY
    }
    fn generation(&self) -> u64 {
        self.observers.generation()
    }
    fn account(&self, user: &str) -> Option<Account> {
        guard(&self.state).accounts.get(user).cloned()
    }
    fn team(&self, id: &str) -> Option<Team> {
        self.team_record(id).map(Team::withholding_codes)
    }
    fn team_as(&self, id: &str, viewer: &str) -> Option<Team> {
        self.team_record(id)
            .map(|t| t.withholding_codes_from(Some(viewer)))
    }
    fn invites_for(&self, user: &str) -> Vec<(String, String, Invite)> {
        let s = guard(&self.state);
        let Some(email) = s.accounts.get(user).map(|a| a.email.to_lowercase()) else {
            return Vec::new();
        };
        s.teams
            .values()
            .flat_map(|t| {
                t.invites
                    .iter()
                    .filter(|i| matches!(&i.to, InviteTo::Email(e) if e.to_lowercase() == email))
                    .map(|i| (t.id.clone(), t.name.clone(), i.clone()))
            })
            .collect()
    }
    fn observe(&self, f: Observer) -> u64 {
        self.observers.add(f)
    }
    fn unobserve(&self, id: u64) {
        self.observers.remove(id);
    }
}

impl IdentityBackend for MemoryIdentity {
    fn team_record(&self, id: &str) -> Option<Team> {
        guard(&self.state).teams.get(id).cloned()
    }
    fn ensure_account(&self, user: &str, email: Option<&str>) {
        let changed = {
            let mut s = guard(&self.state);
            match s.accounts.get_mut(user) {
                Some(a) => match email {
                    Some(e) if a.email != e => {
                        a.email = e.to_string();
                        true
                    }
                    _ => false,
                },
                None => {
                    s.accounts.insert(
                        user.to_string(),
                        Account {
                            user: user.to_string(),
                            display: user.to_string(),
                            email: email
                                .map_or_else(|| format!("{user}@localhost"), str::to_string),
                        },
                    );
                    true
                }
            }
        };
        if changed {
            self.observers.changed();
        }
    }

    fn create_team(&self, name: &str, owner: &str, now_ms: u64) -> Result<Team, ProjectError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(team_err("a team needs a name of 1 to 80 characters"));
        }
        self.edit(|s| {
            s.next += 1;
            let id = format!("team-{}", s.next);
            let t = Team {
                id: id.clone(),
                name: name.to_string(),
                members: vec![Member {
                    user: owner.to_string(),
                    role: Role::Owner,
                    paths: Vec::new(),
                    since_ms: now_ms,
                }],
                invites: Vec::new(),
            };
            s.teams.insert(id, t.clone());
            Ok(t)
        })
    }

    fn invite(
        &self,
        team: &str,
        spec: InviteSpec,
        by: &str,
        now_ms: u64,
    ) -> Result<Invite, ProjectError> {
        self.edit(|s| {
            s.next += 1;
            let n = s.next;
            let known_email = |e: &str| {
                s.accounts
                    .values()
                    .find(|a| a.email.eq_ignore_ascii_case(e))
                    .map(|a| a.user.clone())
            };
            let member_by_email = spec.email.as_deref().and_then(known_email);
            let t = team_mut(s, team)?;
            if let Some(u) = member_by_email
                && t.member(&u).is_some()
            {
                return Err(team_err(format!("{u} is already a member of {}", t.name)));
            }
            let to = match spec.email {
                Some(e) => {
                    let e = e.trim().to_string();
                    if !e.contains('@') || e.len() > 254 {
                        return Err(team_err(format!("{e:?} is not an email address")));
                    }
                    if t.invites
                        .iter()
                        .any(|i| matches!(&i.to, InviteTo::Email(x) if x.eq_ignore_ascii_case(&e)))
                    {
                        return Err(team_err(format!("{e} already has a pending invite")));
                    }
                    InviteTo::Email(e)
                }
                None => {
                    let nanos = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_nanos());
                    InviteTo::Code(mint_code(format!("{team}/{n}/{nanos}").as_bytes()))
                }
            };
            let inv = Invite {
                id: format!("inv-{n}"),
                to,
                role: spec.role,
                paths: spec.paths,
                by: by.to_string(),
                at_ms: now_ms,
            };
            t.invites.push(inv.clone());
            Ok(inv)
        })
    }

    fn revoke_invite(&self, team: &str, invite: &str) -> Result<Invite, ProjectError> {
        self.edit(|s| {
            let t = team_mut(s, team)?;
            let i = t
                .invites
                .iter()
                .position(|i| i.id == invite)
                .ok_or_else(|| team_err(format!("there is no pending invite {invite}")))?;
            Ok(t.invites.remove(i))
        })
    }

    fn accept(&self, user: &str, token: &str, now_ms: u64) -> Result<(Team, Member), ProjectError> {
        let token = token.trim();
        self.edit(|s| {
            let email = s.accounts.get(user).map(|a| a.email.to_lowercase());
            let found = s.teams.iter().find_map(|(tid, t)| {
                t.invites
                    .iter()
                    .position(|i| match &i.to {
                        InviteTo::Code(c) => c.eq_ignore_ascii_case(token),
                        InviteTo::Email(_) => i.id == token,
                    })
                    .map(|p| (tid.clone(), p))
            });
            let (tid, pos) = found.ok_or_else(|| {
                team_err("that join code or invite is not valid (it may have been revoked or used)")
            })?;
            let t = team_mut(s, &tid)?;
            if let InviteTo::Email(e) = &t.invites[pos].to
                && email.as_deref() != Some(e.to_lowercase().as_str())
            {
                return Err(team_err(format!(
                    "this invite is for {e}, not for {user}'s account"
                )));
            }
            if t.member(user).is_some() {
                return Err(team_err(format!(
                    "{user} is already a member of {}",
                    t.name
                )));
            }
            let inv = t.invites.remove(pos);
            let m = Member {
                user: user.to_string(),
                role: inv.role,
                paths: inv.paths,
                since_ms: now_ms,
            };
            t.members.push(m.clone());
            Ok((t.clone(), m))
        })
    }

    fn set_role(&self, team: &str, user: &str, role: Role) -> Result<Role, ProjectError> {
        self.edit(|s| {
            let t = team_mut(s, team)?;
            let owners = t.owners();
            let m = t
                .members
                .iter_mut()
                .find(|m| m.user == user)
                .ok_or_else(|| team_err(format!("{user} is not a member")))?;
            if m.role == Role::Owner && role != Role::Owner && owners <= 1 {
                return Err(team_err(
                    "the team would have no Owner: make someone else an Owner first",
                ));
            }
            Ok(std::mem::replace(&mut m.role, role))
        })
    }

    fn set_paths(&self, team: &str, user: &str, paths: Vec<String>) -> Result<(), ProjectError> {
        self.edit(|s| {
            let t = team_mut(s, team)?;
            let m = t
                .members
                .iter_mut()
                .find(|m| m.user == user)
                .ok_or_else(|| team_err(format!("{user} is not a member")))?;
            m.paths = paths;
            Ok(())
        })
    }

    fn remove_member(&self, team: &str, user: &str) -> Result<Member, ProjectError> {
        self.edit(|s| {
            let t = team_mut(s, team)?;
            let pos = t
                .members
                .iter()
                .position(|m| m.user == user)
                .ok_or_else(|| team_err(format!("{user} is not a member")))?;
            if t.members[pos].role == Role::Owner && t.owners() <= 1 {
                return Err(team_err(
                    "the team would have no Owner: make someone else an Owner first",
                ));
            }
            Ok(t.members.remove(pos))
        })
    }
}

// ---- the baseline, sandboxes, presence, claims, the queue -----------------------------------

/// Who can read a sandbox (O-17: `Team` by default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Visibility {
    /// Its owner and the team's Owners only.
    Private,
    /// Readable by teammates, never writable.
    #[default]
    Team,
    /// Readable by teammates and shared (read-only) for review sessions.
    Shared,
}

impl Visibility {
    pub const ALL: [Visibility; 3] = [Self::Private, Self::Team, Self::Shared];
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Private => "Private",
            Self::Team => "Team",
            Self::Shared => "Shared (read-only)",
        }
    }
}

/// When a sandbox's base advances (I20: two policies over one stream).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Policy {
    /// Automatically, as each publish lands.
    Live,
    /// When you say, up to the revision you choose.
    #[default]
    Pull,
}

impl Policy {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Pull => "Pull",
        }
    }
}

/// A sandbox, as the server keeps it (I19: a row, not a process).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxRow {
    pub owner: String,
    /// The baseline revision it is layered on.
    pub base: Option<RevId>,
    /// Unpublished changes (commands).
    pub unpublished: usize,
    /// Semantic summary of them (`placed 3 “Tree”`).
    pub summary: Vec<String>,
    pub visibility: Visibility,
    pub policy: Policy,
    pub updated_ms: u64,
    /// This row is another person's `Private` sandbox: its content is withheld.
    pub withheld: bool,
}

/// Who is looking at what (session state, published read-only to teammates).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    pub user: String,
    /// Entity keys selected.
    pub selection: Vec<u64>,
    /// The panel in focus (`forge.inspector`).
    pub panel: Option<String>,
    pub at_ms: u64,
}

/// A scoped ownership claim (Ch.37 §37.4): a subtree read-only for everyone but its holder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    /// The subtree's root entity.
    pub entity: u64,
    /// Its content path when claimed (`scene/Levels/One`).
    pub label: String,
    pub holder: String,
    pub at_ms: u64,
}

/// A publish: the sandbox's document, the envelopes that made it, and what it touches.
#[derive(Clone, Debug, PartialEq)]
pub struct Publish {
    pub author: String,
    /// The baseline revision the sandbox is on (must still be the head).
    pub base: Option<RevId>,
    pub message: String,
    /// What the revision **is**: the server commits this document, nothing else.
    pub doc: ProjectDoc,
    /// The revision's story — **metadata only, never replayed** (ADR 0041): the sandbox's
    /// folded deltas (a gesture's frames folded into its last one, undone and cancelled
    /// transactions left out, pulls and loads not in it, keys as they were before a pull
    /// re-keyed an entity). It gives the revision its summary, command count and audit
    /// trail; applying it to the base would not reproduce `doc`.
    pub envelopes: Vec<CommandEnvelope>,
    /// Content paths it changes.
    pub paths: Vec<String>,
    /// Semantic summary.
    pub summary: Vec<String>,
    pub at_ms: u64,
}

/// Where a publish request is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestState {
    Pending,
    Approved {
        by: String,
        rev: RevId,
    },
    Rejected {
        by: String,
        reason: String,
    },
    /// The baseline moved before it was approved: its author pulls and publishes again.
    Stale(String),
}

impl RequestState {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Pending => "waiting for review".into(),
            Self::Approved { by, .. } => format!("approved by {by}"),
            Self::Rejected { by, reason } => format!("rejected by {by}: {reason}"),
            Self::Stale(why) => format!("stale: {why}"),
        }
    }
}

/// A publish request in the review queue (without its document).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub author: String,
    pub base: Option<RevId>,
    pub message: String,
    pub paths: Vec<String>,
    pub summary: Vec<String>,
    pub commands: usize,
    pub state: RequestState,
    pub at_ms: u64,
}

/// One baseline revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaselineRev {
    pub id: RevId,
    pub parent: Option<RevId>,
    pub message: String,
    pub issuers: Vec<String>,
    pub commands: usize,
    pub at_ms: u64,
    pub summary: Vec<String>,
}

/// The collaboration server, read side and session state (panels get this).
pub trait CollabView: Send + Sync {
    /// What serves it (`in-memory stand-in (D-4) over a git baseline`).
    fn backend(&self) -> String;
    /// Bumped on every change.
    fn generation(&self) -> u64;
    /// The team this baseline belongs to.
    fn team(&self) -> Option<String>;
    fn head(&self) -> Option<RevId>;
    /// Baseline revisions after `after` (exclusive; all when `None`) up to the head, oldest
    /// first, at most `limit`.
    fn revisions(&self, after: Option<RevId>, limit: usize) -> Vec<BaselineRev>;
    /// Every sandbox row as `viewer` may see it: a `Private` one of someone else is withheld
    /// unless `viewer` is an Owner (Ch.37 §37.8).
    fn sandboxes(&self, viewer: &str) -> Vec<SandboxRow>;
    fn presence(&self) -> Vec<Presence>;
    fn claims(&self) -> Vec<Claim>;
    fn requests(&self) -> Vec<Request>;
    fn policy(&self, owner: &str) -> Policy;
    fn visibility(&self, owner: &str) -> Visibility;
    /// Session state: who is looking at what.
    fn set_presence(&self, p: Presence);
    fn clear_presence(&self, user: &str);
    /// Session state: `owner`'s own subscription policy (one click, reversible, I20).
    fn set_policy(&self, owner: &str, p: Policy);
    /// Session state: who may read `owner`'s sandbox.
    fn set_visibility(&self, owner: &str, v: Visibility);
    fn observe(&self, f: Observer) -> u64;
    fn unobserve(&self, id: u64);
}

/// The collaboration server, write side: **only the editor core names it**.
pub trait CollabBackend: CollabView {
    /// Bind this baseline to `team` (once; the same team again is a no-op).
    fn bind(&self, team: &str) -> Result<(), ProjectError>;
    /// The baseline document at `rev` (`None`: the empty project before the first revision).
    fn doc_at(&self, rev: Option<RevId>) -> Result<Arc<ProjectDoc>, ProjectError>;
    /// Commit `p` to the baseline (`ProjectStore::commit`): refused with `PROJECT-0016` when
    /// the head is no longer `p.base`.
    fn publish(&self, p: Publish) -> Result<RevId, ProjectError>;
    /// Put `p` in the review queue; its id.
    fn submit(&self, p: Publish) -> Result<u64, ProjectError>;
    /// Approve request `id`: it is published when the baseline has not moved since it was
    /// submitted, else it becomes stale (the error says so).
    fn approve(&self, id: u64, by: &str) -> Result<RevId, ProjectError>;
    fn reject(&self, id: u64, by: &str, reason: &str) -> Result<(), ProjectError>;
    /// Take a claim: refused (`PROJECT-0017`) when someone else holds that entity.
    fn claim(&self, c: Claim) -> Result<(), ProjectError>;
    /// Release the claim on `entity` (the holder's own, or anyone's with `force`).
    fn release(&self, entity: u64, by: &str, force: bool) -> Result<Claim, ProjectError>;
    /// Release every claim `holder` has (a removed member).
    fn release_all(&self, holder: &str) -> Vec<Claim>;
    /// Record `owner`'s sandbox row (its base, what is unpublished).
    fn record_sandbox(
        &self,
        owner: &str,
        base: Option<RevId>,
        unpublished: usize,
        summary: Vec<String>,
        now_ms: u64,
    );
}

/// The in-memory collaboration server (D-4, labelled) over a **real** baseline store: a
/// `MemoryStore`, a `LocalFs` folder or a Git repository — whatever [`ProjectStore`] it is
/// given. Sandbox rows, presence, claims and the queue live in memory as long as the process.
pub struct MemoryCollab {
    store: Mutex<Box<dyn ProjectStore>>,
    identity: Arc<dyn IdentityView>,
    state: Mutex<CollabState>,
    observers: Observers,
}

#[derive(Default)]
struct CollabState {
    team: Option<String>,
    sandboxes: BTreeMap<String, SandboxRow>,
    presence: BTreeMap<String, Presence>,
    claims: BTreeMap<u64, Claim>,
    requests: Vec<(Request, Option<Publish>)>,
    next_request: u64,
    docs: BTreeMap<RevId, Arc<ProjectDoc>>,
    summaries: BTreeMap<RevId, Vec<String>>,
}

/// Documents kept decoded (the heads sandboxes pull from); older ones are re-read.
const DOC_CACHE: usize = 32;

impl MemoryCollab {
    /// A server whose baseline is `store`, checking roles in `identity`.
    #[must_use]
    pub fn new(store: Box<dyn ProjectStore>, identity: Arc<dyn IdentityView>) -> Self {
        Self {
            store: Mutex::new(store),
            identity,
            state: Mutex::new(CollabState::default()),
            observers: Observers::default(),
        }
    }

    fn role(&self, team: Option<&str>, user: &str) -> Option<crate::collab::Role> {
        team.and_then(|t| self.identity.team(t))
            .and_then(|t| t.role_of(user))
    }

    fn row_mut<'a>(s: &'a mut CollabState, owner: &str) -> &'a mut SandboxRow {
        s.sandboxes
            .entry(owner.to_string())
            .or_insert_with(|| SandboxRow {
                owner: owner.to_string(),
                base: None,
                unpublished: 0,
                summary: Vec::new(),
                visibility: Visibility::default(),
                policy: Policy::default(),
                updated_ms: 0,
                withheld: false,
            })
    }

    /// Commit `p` if the head is still its base (under the store lock).
    fn commit(&self, p: &Publish) -> Result<RevId, ProjectError> {
        let mut store = guard(&self.store);
        let head = store.head()?;
        if head != p.base {
            return Err(ProjectError::BaselineMoved(format!(
                "the baseline moved since this sandbox's base ({} now, {} then): pull, then publish",
                short(head),
                short(p.base)
            )));
        }
        for (path, text) in p.doc.encode()? {
            store.write(&StorePath::new(path)?, Bytes::from(text))?;
        }
        let rev = store.commit(&p.message, &p.envelopes)?;
        store.publish()?;
        drop(store);
        let mut s = guard(&self.state);
        s.docs.insert(rev, Arc::new(p.doc.clone()));
        while s.docs.len() > DOC_CACHE {
            s.docs.pop_first();
        }
        s.summaries.insert(rev, p.summary.clone());
        Ok(rev)
    }
}

/// A revision's first ten hex digits (`—` for none).
#[must_use]
pub fn short(rev: Option<RevId>) -> String {
    rev.map_or_else(
        || "\u{2014}".to_string(),
        |r| r.to_string().chars().take(10).collect(),
    )
}

impl CollabView for MemoryCollab {
    fn backend(&self) -> String {
        format!(
            "{IN_MEMORY} over a {} baseline",
            guard(&self.store).backend()
        )
    }
    fn generation(&self) -> u64 {
        self.observers.generation()
    }
    fn team(&self) -> Option<String> {
        guard(&self.state).team.clone()
    }
    fn head(&self) -> Option<RevId> {
        guard(&self.store).head().ok().flatten()
    }
    fn revisions(&self, after: Option<RevId>, limit: usize) -> Vec<BaselineRev> {
        let store = guard(&self.store);
        let Ok(mut revs) = store.history(RevRange {
            to: None,
            stop_at: after,
            limit: None,
        }) else {
            return Vec::new();
        };
        revs.reverse();
        revs.truncate(limit);
        let cached = guard(&self.state).summaries.clone();
        revs.into_iter()
            .map(|r| {
                let summary = cached.get(&r.id).cloned().unwrap_or_else(|| {
                    store
                        .commands(&r.id)
                        .map(|c| crate::summary::summarize(&c))
                        .unwrap_or_default()
                });
                BaselineRev {
                    id: r.id,
                    parent: r.parent,
                    message: r.message,
                    issuers: r.issuers,
                    commands: r.commands,
                    at_ms: r.at_ms,
                    summary,
                }
            })
            .collect()
    }
    fn sandboxes(&self, viewer: &str) -> Vec<SandboxRow> {
        let s = guard(&self.state);
        let reads_private = self
            .role(s.team.as_deref(), viewer)
            .is_some_and(Role::reads_private);
        s.sandboxes
            .values()
            .map(|r| {
                if r.visibility == Visibility::Private && r.owner != viewer && !reads_private {
                    SandboxRow {
                        owner: r.owner.clone(),
                        base: None,
                        unpublished: 0,
                        summary: Vec::new(),
                        visibility: r.visibility,
                        policy: r.policy,
                        updated_ms: r.updated_ms,
                        withheld: true,
                    }
                } else {
                    r.clone()
                }
            })
            .collect()
    }
    fn presence(&self) -> Vec<Presence> {
        guard(&self.state).presence.values().cloned().collect()
    }
    fn claims(&self) -> Vec<Claim> {
        guard(&self.state).claims.values().cloned().collect()
    }
    fn requests(&self) -> Vec<Request> {
        guard(&self.state)
            .requests
            .iter()
            .map(|(r, _)| r.clone())
            .collect()
    }
    fn policy(&self, owner: &str) -> Policy {
        guard(&self.state)
            .sandboxes
            .get(owner)
            .map_or_else(Policy::default, |r| r.policy)
    }
    fn visibility(&self, owner: &str) -> Visibility {
        guard(&self.state)
            .sandboxes
            .get(owner)
            .map_or_else(Visibility::default, |r| r.visibility)
    }
    fn set_presence(&self, p: Presence) {
        {
            let mut s = guard(&self.state);
            if s.presence
                .get(&p.user)
                .is_some_and(|old| old.selection == p.selection && old.panel == p.panel)
            {
                return;
            }
            s.presence.insert(p.user.clone(), p);
        }
        self.observers.changed();
    }
    fn clear_presence(&self, user: &str) {
        let removed = guard(&self.state).presence.remove(user).is_some();
        if removed {
            self.observers.changed();
        }
    }
    fn set_policy(&self, owner: &str, p: Policy) {
        {
            let mut s = guard(&self.state);
            let r = Self::row_mut(&mut s, owner);
            if r.policy == p {
                return;
            }
            r.policy = p;
        }
        self.observers.changed();
    }
    fn set_visibility(&self, owner: &str, v: Visibility) {
        {
            let mut s = guard(&self.state);
            let r = Self::row_mut(&mut s, owner);
            if r.visibility == v {
                return;
            }
            r.visibility = v;
        }
        self.observers.changed();
    }
    fn observe(&self, f: Observer) -> u64 {
        self.observers.add(f)
    }
    fn unobserve(&self, id: u64) {
        self.observers.remove(id);
    }
}

impl CollabBackend for MemoryCollab {
    fn bind(&self, team: &str) -> Result<(), ProjectError> {
        {
            let mut s = guard(&self.state);
            match &s.team {
                Some(t) if t == team => return Ok(()),
                Some(t) => {
                    return Err(ProjectError::Team(format!(
                        "this baseline belongs to {t}, not {team}"
                    )));
                }
                None => s.team = Some(team.to_string()),
            }
        }
        self.observers.changed();
        Ok(())
    }

    fn doc_at(&self, rev: Option<RevId>) -> Result<Arc<ProjectDoc>, ProjectError> {
        let Some(rev) = rev else {
            return Ok(Arc::new(ProjectDoc::default()));
        };
        if let Some(d) = guard(&self.state).docs.get(&rev) {
            return Ok(Arc::clone(d));
        }
        let store = guard(&self.store);
        let tree = store.tree(&rev)?;
        let read = |path: &str| -> Result<Option<String>, ProjectError> {
            match tree.entries.iter().find(|(p, _)| p.as_str() == path) {
                Some((_, h)) => {
                    let b = store.blob_get(*h)?;
                    String::from_utf8(b.to_vec())
                        .map(Some)
                        .map_err(|e| ProjectError::BadFiles(format!("{path}: {e}")))
                }
                None => Ok(None),
            }
        };
        let manifest = read(MANIFEST_PATH)?
            .ok_or_else(|| ProjectError::BadFiles(format!("{MANIFEST_PATH} is missing")))?;
        let (_, doc) = ProjectDoc::decode(
            &manifest,
            read(SETTINGS_PATH)?.as_deref(),
            read(SCENE_PATH)?.as_deref(),
        )?;
        drop(store);
        let doc = Arc::new(doc);
        let mut s = guard(&self.state);
        s.docs.insert(rev, Arc::clone(&doc));
        while s.docs.len() > DOC_CACHE {
            s.docs.pop_first();
        }
        Ok(doc)
    }

    fn publish(&self, p: Publish) -> Result<RevId, ProjectError> {
        let rev = self.commit(&p)?;
        self.observers.changed();
        Ok(rev)
    }

    fn submit(&self, p: Publish) -> Result<u64, ProjectError> {
        let id = {
            let mut s = guard(&self.state);
            s.next_request += 1;
            let id = s.next_request;
            let r = Request {
                id,
                author: p.author.clone(),
                base: p.base,
                message: p.message.clone(),
                paths: p.paths.clone(),
                summary: p.summary.clone(),
                commands: p.envelopes.len(),
                state: RequestState::Pending,
                at_ms: p.at_ms,
            };
            // A newer request from the same author supersedes a pending one.
            for (old, body) in &mut s.requests {
                if old.author == p.author && old.state == RequestState::Pending {
                    old.state = RequestState::Stale(format!("superseded by request #{id}"));
                    *body = None;
                }
            }
            s.requests.push((r, Some(p)));
            id
        };
        self.observers.changed();
        Ok(id)
    }

    fn approve(&self, id: u64, by: &str) -> Result<RevId, ProjectError> {
        let p = {
            let s = guard(&self.state);
            let (r, body) = s
                .requests
                .iter()
                .find(|(r, _)| r.id == id)
                .ok_or_else(|| ProjectError::Team(format!("there is no request #{id}")))?;
            if r.state != RequestState::Pending {
                return Err(ProjectError::Team(format!(
                    "request #{id} is {}, not waiting for review",
                    r.state.label()
                )));
            }
            body.clone()
                .ok_or_else(|| ProjectError::Team(format!("request #{id} has no content")))?
        };
        let result = self.commit(&p);
        {
            let mut s = guard(&self.state);
            if let Some((r, body)) = s.requests.iter_mut().find(|(r, _)| r.id == id) {
                match &result {
                    Ok(rev) => {
                        r.state = RequestState::Approved {
                            by: by.to_string(),
                            rev: *rev,
                        };
                        *body = None;
                    }
                    Err(ProjectError::BaselineMoved(_)) => {
                        r.state = RequestState::Stale(
                            "the baseline moved since it was submitted: its author pulls and publishes again".into(),
                        );
                        *body = None;
                    }
                    Err(_) => {}
                }
            }
        }
        self.observers.changed();
        result
    }

    fn reject(&self, id: u64, by: &str, reason: &str) -> Result<(), ProjectError> {
        {
            let mut s = guard(&self.state);
            let (r, body) = s
                .requests
                .iter_mut()
                .find(|(r, _)| r.id == id)
                .ok_or_else(|| ProjectError::Team(format!("there is no request #{id}")))?;
            if r.state != RequestState::Pending {
                return Err(ProjectError::Team(format!(
                    "request #{id} is {}, not waiting for review",
                    r.state.label()
                )));
            }
            r.state = RequestState::Rejected {
                by: by.to_string(),
                reason: reason.to_string(),
            };
            *body = None;
        }
        self.observers.changed();
        Ok(())
    }

    fn claim(&self, c: Claim) -> Result<(), ProjectError> {
        {
            let mut s = guard(&self.state);
            if let Some(old) = s.claims.get(&c.entity) {
                if old.holder == c.holder {
                    return Ok(());
                }
                return Err(ProjectError::Claimed(format!(
                    "\u{201c}{}\u{201d} is claimed by {}",
                    old.label, old.holder
                )));
            }
            s.claims.insert(c.entity, c);
        }
        self.observers.changed();
        Ok(())
    }

    fn release(&self, entity: u64, by: &str, force: bool) -> Result<Claim, ProjectError> {
        let c = {
            let mut s = guard(&self.state);
            let old = s
                .claims
                .get(&entity)
                .ok_or_else(|| ProjectError::Team(format!("e{entity} is not claimed")))?;
            if old.holder != by && !force {
                return Err(ProjectError::Claimed(format!(
                    "\u{201c}{}\u{201d} is {}'s claim: only they (or a Maintainer) release it",
                    old.label, old.holder
                )));
            }
            s.claims.remove(&entity)
        };
        self.observers.changed();
        c.ok_or_else(|| ProjectError::Team(format!("e{entity} is not claimed")))
    }

    fn release_all(&self, holder: &str) -> Vec<Claim> {
        let gone: Vec<Claim> = {
            let mut s = guard(&self.state);
            let keys: Vec<u64> = s
                .claims
                .values()
                .filter(|c| c.holder == holder)
                .map(|c| c.entity)
                .collect();
            keys.iter().filter_map(|k| s.claims.remove(k)).collect()
        };
        if !gone.is_empty() {
            self.observers.changed();
        }
        gone
    }

    fn record_sandbox(
        &self,
        owner: &str,
        base: Option<RevId>,
        unpublished: usize,
        summary: Vec<String>,
        now_ms: u64,
    ) {
        {
            let mut s = guard(&self.state);
            let r = Self::row_mut(&mut s, owner);
            if r.base == base && r.unpublished == unpublished && r.summary == summary {
                return;
            }
            r.base = base;
            r.unpublished = unpublished;
            r.summary = summary;
            r.updated_ms = now_ms;
        }
        self.observers.changed();
    }
}

/// Every member's scopes and claims, for the member list: `user -> claimed labels`.
#[must_use]
pub fn claims_by_holder(claims: &[Claim]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in claims {
        out.entry(c.holder.clone())
            .or_default()
            .push(c.label.clone());
    }
    out
}

/// The users present (for a set of entity keys): `key -> users looking at it`, excluding
/// `me`.
#[must_use]
pub fn presence_by_entity(presence: &[Presence], me: &str) -> BTreeMap<u64, BTreeSet<String>> {
    let mut out: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
    for p in presence.iter().filter(|p| p.user != me) {
        for k in &p.selection {
            out.entry(*k).or_default().insert(p.user.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::EntityDoc;

    fn server() -> (Arc<MemoryIdentity>, MemoryCollab) {
        let id = Arc::new(
            MemoryIdentity::new()
                .with_account("ada", "ada@studio.example")
                .with_account("bob", "bob@studio.example"),
        );
        static N: AtomicU64 = AtomicU64::new(0);
        let store = crate::memory::open_named(
            &format!(
                "collab-unit-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ),
            "forge-server",
        );
        let c = MemoryCollab::new(store, id.clone());
        (id, c)
    }

    fn doc(names: &[&str]) -> ProjectDoc {
        ProjectDoc {
            settings: Default::default(),
            entities: names
                .iter()
                .enumerate()
                .map(|(i, n)| EntityDoc {
                    id: i as u64,
                    name: (*n).into(),
                    parent: None,
                    properties: Default::default(),
                })
                .collect(),
        }
    }

    fn publish(author: &str, base: Option<RevId>, d: ProjectDoc) -> Publish {
        Publish {
            author: author.into(),
            base,
            message: format!("{author} publishes"),
            doc: d,
            envelopes: Vec::new(),
            paths: Vec::new(),
            summary: Vec::new(),
            at_ms: 1,
        }
    }

    #[test]
    fn roles_never_escalate_past_their_own() {
        use Role::*;
        assert!(may_assign(Some(Owner), Some(Developer), Owner).is_ok());
        assert!(may_assign(Some(Maintainer), Some(Viewer), Developer).is_ok());
        assert!(may_assign(Some(Maintainer), Some(Developer), Maintainer).is_err());
        assert!(may_assign(Some(Maintainer), Some(Owner), Viewer).is_err());
        assert!(may_assign(Some(Maintainer), None, Owner).is_err());
        assert!(may_assign(Some(Developer), None, Viewer).is_err());
        assert!(may_assign(None, None, Viewer).is_err());
    }

    #[test]
    fn a_code_invite_is_accepted_once_and_an_email_invite_only_by_its_account() {
        let (id, _) = server();
        let t = id
            .create_team("Studio", "ada", 1)
            .unwrap_or_else(|e| panic!("{e}"));
        let code = id
            .invite(
                &t.id,
                InviteSpec {
                    email: None,
                    role: Role::Developer,
                    paths: vec!["scene/Levels/**".into()],
                },
                "ada",
                2,
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let InviteTo::Code(c) = &code.to else {
            panic!("a code invite")
        };
        assert_eq!(c.len(), 9);
        let (_, m) = id.accept("bob", c, 3).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((m.role, m.paths.len()), (Role::Developer, 1));
        assert!(id.accept("carol", c, 4).is_err(), "a used code is gone");
        let mail = id
            .invite(
                &t.id,
                InviteSpec {
                    email: Some("carol@studio.example".into()),
                    role: Role::Viewer,
                    paths: Vec::new(),
                },
                "ada",
                5,
            )
            .unwrap_or_else(|e| panic!("{e}"));
        id.ensure_account("mallory", Some("mallory@x.example"));
        assert!(id.accept("mallory", &mail.id, 6).is_err());
        id.ensure_account("carol", Some("carol@studio.example"));
        assert_eq!(id.invites_for("carol").len(), 1);
        assert!(id.accept("carol", &mail.id, 7).is_ok());
        // The last Owner stays.
        assert!(id.set_role(&t.id, "ada", Role::Developer).is_err());
        assert!(id.remove_member(&t.id, "ada").is_err());
    }

    #[test]
    fn publish_is_a_fast_forward_and_a_moved_baseline_is_refused() {
        let (_, c) = server();
        let r1 = c
            .publish(publish("ada", c.head(), doc(&["World"])))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.head(), Some(r1));
        assert_eq!(c.doc_at(Some(r1)).map(|d| d.entities.len()).ok(), Some(1));
        // Bob is still on the empty baseline: refused, nothing committed.
        let r = c.publish(publish("bob", None, doc(&["Other"])));
        assert!(matches!(r, Err(ProjectError::BaselineMoved(_))), "{r:?}");
        assert_eq!(c.head(), Some(r1));
        assert_eq!(c.revisions(None, 10).len(), 1);
    }

    #[test]
    fn the_queue_approves_rejects_and_marks_stale() {
        let (_, c) = server();
        let r1 = c
            .publish(publish("ada", None, doc(&["World"])))
            .unwrap_or_else(|e| panic!("{e}"));
        let a = c
            .submit(publish("bob", Some(r1), doc(&["World", "Tree"])))
            .unwrap_or_else(|e| panic!("{e}"));
        let b = c
            .submit(publish("carol", Some(r1), doc(&["World", "Rock"])))
            .unwrap_or_else(|e| panic!("{e}"));
        let r2 = c.approve(a, "ada").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.head(), Some(r2));
        // Carol's is now behind: approving it does not overwrite bob's work.
        assert!(c.approve(b, "ada").is_err());
        let states: Vec<RequestState> = c.requests().into_iter().map(|r| r.state).collect();
        assert!(matches!(states[0], RequestState::Approved { .. }));
        assert!(matches!(states[1], RequestState::Stale(_)));
        let d = c
            .submit(publish("dan", Some(r2), doc(&["World"])))
            .unwrap_or_else(|e| panic!("{e}"));
        c.reject(d, "ada", "not yet")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.head(), Some(r2));
    }

    #[test]
    fn a_private_sandbox_is_withheld_from_everyone_but_its_owner_and_owners() {
        let (id, c) = server();
        let t = id
            .create_team("Studio", "ada", 1)
            .unwrap_or_else(|e| panic!("{e}"));
        c.bind(&t.id).unwrap_or_else(|e| panic!("{e}"));
        let code = id
            .invite(
                &t.id,
                InviteSpec {
                    email: None,
                    role: Role::Developer,
                    paths: Vec::new(),
                },
                "ada",
                2,
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let InviteTo::Code(code) = code.to else {
            panic!("code")
        };
        id.accept("bob", &code, 3).unwrap_or_else(|e| panic!("{e}"));
        id.ensure_account("carol", None);
        let code = id
            .invite(
                &t.id,
                InviteSpec {
                    email: None,
                    role: Role::Developer,
                    paths: Vec::new(),
                },
                "ada",
                4,
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let InviteTo::Code(code) = code.to else {
            panic!("code")
        };
        id.accept("carol", &code, 5)
            .unwrap_or_else(|e| panic!("{e}"));
        c.record_sandbox("bob", None, 3, vec!["placed 3 “Tree”".into()], 6);
        c.set_visibility("bob", Visibility::Private);
        let as_seen = |viewer: &str| {
            c.sandboxes(viewer)
                .into_iter()
                .find(|r| r.owner == "bob")
                .map(|r| (r.withheld, r.unpublished))
        };
        assert_eq!(as_seen("carol"), Some((true, 0)), "a teammate sees nothing");
        assert_eq!(as_seen("bob"), Some((false, 3)), "the owner sees it");
        assert_eq!(as_seen("ada"), Some((false, 3)), "the Owner role sees it");
        c.set_visibility("bob", Visibility::Team);
        assert_eq!(as_seen("carol"), Some((false, 3)));
    }

    #[test]
    fn a_claim_is_held_by_one_person_and_released_by_them_or_forced() {
        let (_, c) = server();
        let claim = |holder: &str| Claim {
            entity: 4,
            label: "scene/Level".into(),
            holder: holder.into(),
            at_ms: 1,
        };
        c.claim(claim("bob")).unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(
            c.claim(claim("ada")),
            Err(ProjectError::Claimed(_))
        ));
        assert!(c.release(4, "ada", false).is_err());
        assert!(c.release(4, "ada", true).is_ok());
        c.claim(claim("ada")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.release_all("ada").len(), 1);
        assert!(c.claims().is_empty());
    }
}
