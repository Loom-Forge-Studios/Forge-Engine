//! The editor's audit trail (Ch.21 §21.21 "Audit log", Ch.34 §34.5, Ch.37; DoD M2-54):
//! **every automation, remote and teammate envelope, greppable by session, user and
//! sandbox**, and every security command and device pairing.
//!
//! Two kinds of source feed the audit panel through [`AuditSource`]:
//!
//! * the editor's [`AuditBook`]: what the **core** writes as it applies security commands
//!   (plugin and automation grants and revokes, the default automation policy, plugin-set
//!   changes, approvals and rejections of automation work — read from the applied diffs, so
//!   an undo that revokes is there too), and what the
//!   [`RemotePairingStore`](super::remote::RemotePairingStore) writes for every pairing, unpairing,
//!   exposure change and remote session;
//! * an [`AuditFeed`] a plugin attaches when it hosts its own sessions over the core (a
//!   protocol server's log: every envelope, refusal and denial it handled, keyed by session),
//!   read live, merged into one timeline with the book.
//!
//! Teammate envelopes (Ch.37) have no producer yet: `forge-collab` is not built (D-4); the
//! panel says so rather than showing an empty list as if nothing happened.
//!
//! **Grep.** [`AuditQuery`] is `field:value` terms (`session:auto-3`, `user:ada`,
//! `sandbox:alice`, `origin:security`, `event:grant`) that must match exactly, and bare words
//! that must appear, case-insensitively, anywhere in the line. `session:auto-1` does not
//! match `auto-10`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use forge_cmd::{Clock, Issuer, SystemClock};
use serde::Serialize;

/// Records a book keeps (the oldest go first).
pub const BOOK_CAPACITY: usize = 65_536;

/// Where an audit record came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum AuditOrigin {
    /// An automation session a plugin hosts over the core (its [`AuditFeed`]).
    Automation,
    /// A security command the core applied (grants, policy, plugin set, approvals).
    Security,
    /// Remote connect: pairing, exposure, remote sessions.
    Remote,
    /// A teammate's envelope (Ch.37): a baseline revision pulled into this sandbox.
    Teammate,
    /// Team management, claims, publishing and review (Ch.37, WP-U10): what the core
    /// performed for a collaboration command.
    Team,
}

impl AuditOrigin {
    /// Its lower-case name (`origin:` in a query).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Automation => "automation",
            Self::Security => "security",
            Self::Remote => "remote",
            Self::Teammate => "teammate",
            Self::Team => "team",
        }
    }
}

/// One audit record (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AuditRecord {
    /// Order within its source.
    pub n: u64,
    /// Wall clock, ms since the Unix epoch (provenance, never content).
    pub at_ms: u64,
    pub origin: AuditOrigin,
    /// The session: an automation session (`auto-3`), a remote session, or the session a
    /// security command concerns. Empty when none.
    pub session: String,
    /// The user: who issued it (`ada` for `human:ada`), or the paired device's owner.
    pub user: String,
    /// The sandbox (Ch.37) it happened in; empty for the local project.
    pub sandbox: String,
    /// What happened (`grant`, `revoke`, `command`, `denied`, `paired`, `approve`).
    pub event: String,
    /// The rest, human-readable.
    pub detail: String,
}

impl AuditRecord {
    /// One line, as the panel shows it and as `grep` sees it.
    // l10n-block: the audit record's stable, greppable form (field names are its format)
    #[must_use]
    pub fn line(&self) -> String {
        let mut s = format!("[{}] {}", self.origin.name(), self.event);
        if !self.session.is_empty() {
            s.push_str(&format!(" session:{}", self.session));
        }
        if !self.user.is_empty() {
            s.push_str(&format!(" user:{}", self.user));
        }
        if !self.sandbox.is_empty() {
            s.push_str(&format!(" sandbox:{}", self.sandbox));
        }
        if !self.detail.is_empty() {
            s.push_str(" \u{2014} ");
            s.push_str(&self.detail);
        }
        s
    }

    /// The user an issuer names (`human:ada` → `ada`, `script:x` → `script:x`).
    #[must_use]
    pub fn user_of(i: &Issuer) -> String {
        match i {
            Issuer::Human { user } => user.clone(),
            other => other.tag(),
        }
    }
}

/// A grep over audit records (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditQuery {
    pub session: Option<String>,
    pub user: Option<String>,
    pub sandbox: Option<String>,
    pub origin: Option<String>,
    pub event: Option<String>,
    /// Lower-cased words that must all appear in the line.
    pub words: Vec<String>,
}

impl AuditQuery {
    /// Parse `session:auto-3 user:ada word …`.
    #[must_use]
    pub fn parse(q: &str) -> Self {
        let mut out = Self::default();
        for t in q.split_whitespace() {
            match t.split_once(':') {
                Some(("session", v)) if !v.is_empty() => out.session = Some(v.to_string()),
                Some(("user", v)) if !v.is_empty() => out.user = Some(v.to_string()),
                Some(("sandbox", v)) if !v.is_empty() => out.sandbox = Some(v.to_string()),
                Some(("origin", v)) if !v.is_empty() => out.origin = Some(v.to_lowercase()),
                Some(("event", v)) if !v.is_empty() => out.event = Some(v.to_string()),
                _ => out.words.push(t.to_lowercase()),
            }
        }
        out
    }

    /// Whether the query is empty (everything matches).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether `r` matches.
    #[must_use]
    pub fn matches(&self, r: &AuditRecord) -> bool {
        let eq = |want: &Option<String>, have: &str| want.as_deref().is_none_or(|w| w == have);
        if !(eq(&self.session, &r.session)
            && eq(&self.user, &r.user)
            && eq(&self.sandbox, &r.sandbox)
            && eq(&self.origin, r.origin.name())
            && eq(&self.event, &r.event))
        {
            return false;
        }
        if self.words.is_empty() {
            return true;
        }
        let line = r.line().to_lowercase();
        self.words.iter().all(|w| line.contains(w.as_str()))
    }
}

type Observer = Arc<dyn Fn() + Send + Sync>;

struct Book {
    kept: VecDeque<AuditRecord>,
    next: u64,
    dropped: u64,
    clock: Box<dyn Clock>,
    observer: Option<Observer>,
}

/// The editor's shared audit book (see the module docs). Cheap to clone; every clone is
/// the same book. Safe to write from any thread.
#[derive(Clone)]
pub struct AuditBook {
    inner: Arc<Mutex<Book>>,
    generation: Arc<AtomicU64>,
}

impl std::fmt::Debug for AuditBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditBook")
            .field("generation", &self.generation())
            .finish_non_exhaustive()
    }
}

impl Default for AuditBook {
    fn default() -> Self {
        Self::new(Box::new(SystemClock))
    }
}

impl AuditBook {
    /// An empty book stamping records with `clock`.
    #[must_use]
    pub fn new(clock: Box<dyn Clock>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Book {
                kept: VecDeque::new(),
                next: 0,
                dropped: 0,
                clock,
                observer: None,
            })),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Book> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Write a record (numbered and stamped here).
    pub fn record(
        &self,
        origin: AuditOrigin,
        session: &str,
        user: &str,
        event: &str,
        detail: impl Into<String>,
    ) {
        self.record_in(origin, session, user, "", event, detail);
    }

    /// Write a record that happened in `sandbox` (Ch.37: greppable by `sandbox:`).
    pub fn record_in(
        &self,
        origin: AuditOrigin,
        session: &str,
        user: &str,
        sandbox: &str,
        event: &str,
        detail: impl Into<String>,
    ) {
        let observer = {
            let mut b = self.lock();
            let r = AuditRecord {
                n: b.next,
                at_ms: b.clock.now_ms(),
                origin,
                session: session.to_string(),
                user: user.to_string(),
                sandbox: sandbox.to_string(),
                event: event.to_string(),
                detail: detail.into(),
            };
            b.next += 1;
            if b.kept.len() >= BOOK_CAPACITY {
                b.kept.pop_front();
                b.dropped += 1;
            }
            b.kept.push_back(r);
            b.observer.clone()
        };
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(o) = observer {
            o();
        }
    }

    /// Every kept record matching `q`, oldest first.
    #[must_use]
    pub fn query(&self, q: &AuditQuery) -> Vec<AuditRecord> {
        self.query_from(q, 0)
    }

    /// The kept records numbered `n` or later that match `q`, oldest first (binary search to
    /// the first: following the book costs what is new).
    #[must_use]
    pub fn query_from(&self, q: &AuditQuery, n: u64) -> Vec<AuditRecord> {
        let b = self.lock();
        let at = b.kept.partition_point(|r| r.n < n);
        b.kept
            .range(at..)
            .filter(|r| q.matches(r))
            .cloned()
            .collect()
    }

    /// The book's clock now (ms since the Unix epoch).
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.lock().clock.now_ms()
    }

    /// Records written so far.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.lock().next
    }

    /// True when nothing was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Records the book let go of (never silent: the panel shows the count).
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.lock().dropped
    }

    /// Bumped on every record.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Be told of every record (the panel's live cell); `None` stops it.
    pub fn set_observer(&self, o: Option<Observer>) {
        self.lock().observer = o;
    }
}

/// Where a reader of an [`AuditSource`] is: the next record number of each source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuditCursor {
    pub book: u64,
    /// The next record number of the attached [`AuditFeed`], if any.
    pub feed: u64,
}

/// What the audit panel reads (see the module docs).
pub trait AuditSource {
    /// What it reads, for the panel's footer.
    fn backend(&self) -> String;
    /// Changes whenever a new record may be there.
    fn generation(&self) -> u64;
    /// Every kept record matching `q`, oldest first, across every source.
    fn query(&self, q: &AuditQuery) -> Vec<AuditRecord> {
        self.query_from(q, AuditCursor::default()).0
    }
    /// The records after `from` matching `q`, oldest first, and the cursor after them: a
    /// panel following the log reads only what is new.
    fn query_from(&self, q: &AuditQuery, from: AuditCursor) -> (Vec<AuditRecord>, AuditCursor);
    /// Records a source let go of because it was full.
    fn dropped(&self) -> u64 {
        0
    }
    /// A live source bumped when a record arrives that is not written to the core's book
    /// (an attached [`AuditFeed`]'s): the audit panel follows it while visible.
    fn live(&self) -> Option<Arc<dyn forge_ui::LiveSource>> {
        None
    }
}

/// A log kept outside the core's [`AuditBook`] that joins the audit timeline: what a plugin
/// that hosts its own sessions over the core (a protocol server) records for every envelope,
/// refusal and denial it handles. The editor knows nothing of the server: the plugin attaches
/// the feed ([`super::ConnectServices::attach_audit_feed`]) and the panel reads it through
/// [`EditorAudit`] like the book.
pub trait AuditFeed: Send + Sync {
    /// What it is, for the panel's footer (a localisation key or plain text).
    fn backend(&self) -> String;
    /// Changes whenever a new record may be there.
    fn generation(&self) -> u64;
    /// The records numbered `from` or later that match `q`, appended to `out`, oldest
    /// first; the next number. A feed that knows `q` cannot match any of its records (a
    /// query naming a sandbox, another origin) returns the next number without walking.
    fn records_from(&self, q: &AuditQuery, from: u64, out: &mut Vec<AuditRecord>) -> u64;
    /// Bumped when a record arrives (see [`AuditSource::live`]).
    fn live(&self) -> Option<Arc<dyn forge_ui::LiveSource>> {
        None
    }
}

/// The editor's audit: its book and, when a plugin attached one, an [`AuditFeed`].
pub struct EditorAudit {
    pub book: AuditBook,
    pub feed: Option<Arc<dyn AuditFeed>>,
    #[doc(hidden)]
    pub skip_security: bool,
}

impl std::fmt::Debug for EditorAudit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorAudit")
            .field("book", &self.book)
            .field("feed", &self.feed.is_some())
            .finish()
    }
}

impl AuditSource for EditorAudit {
    fn backend(&self) -> String {
        let book = "the editor's security and remote book \u{b7} teammate envelopes: UNBUILT (forge-collab, D-4)";
        match &self.feed {
            Some(f) => format!("{} + {book}", f.backend()),
            None => book.to_string(),
        }
    }

    fn generation(&self) -> u64 {
        self.book.generation() + self.feed.as_ref().map_or(0, |f| f.generation())
    }

    fn query_from(&self, q: &AuditQuery, from: AuditCursor) -> (Vec<AuditRecord>, AuditCursor) {
        let book_next = self.book.len();
        let mut out: Vec<AuditRecord> = self
            .book
            .query_from(q, from.book)
            .into_iter()
            .filter(|r| r.n < book_next)
            .filter(|r| !(self.skip_security && r.origin == AuditOrigin::Security))
            .collect();
        let feed_next = match &self.feed {
            Some(f) => f.records_from(q, from.feed, &mut out),
            None => from.feed,
        };
        // One timeline: by time, then source, then order within the source.
        out.sort_by_key(|r| (r.at_ms, r.origin, r.n));
        (
            out,
            AuditCursor {
                book: book_next,
                feed: feed_next,
            },
        )
    }

    fn dropped(&self) -> u64 {
        self.book.dropped()
    }

    fn live(&self) -> Option<Arc<dyn forge_ui::LiveSource>> {
        self.feed.as_ref().and_then(|f| f.live())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(session: &str, user: &str, event: &str, detail: &str) -> AuditRecord {
        AuditRecord {
            n: 0,
            at_ms: 0,
            origin: AuditOrigin::Security,
            session: session.into(),
            user: user.into(),
            sandbox: String::new(),
            event: event.into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn a_query_matches_fields_exactly_and_words_anywhere() {
        let r = rec(
            "auto-1",
            "ada",
            "grant",
            "Command(Destructive) to automation:auto-1",
        );
        assert!(AuditQuery::parse("session:auto-1").matches(&r));
        assert!(!AuditQuery::parse("session:auto-10").matches(&r));
        assert!(
            !AuditQuery::parse("session:auto").matches(&r),
            "exact, not a prefix"
        );
        assert!(AuditQuery::parse("user:ada destructive").matches(&r));
        assert!(!AuditQuery::parse("user:bob").matches(&r));
        assert!(AuditQuery::parse("origin:security event:grant").matches(&r));
        assert!(!AuditQuery::parse("origin:automation").matches(&r));
        assert!(AuditQuery::parse("").is_empty());
        assert!(AuditQuery::parse("COMMAND(destructive)").matches(&r));
    }

    #[test]
    fn a_book_is_bounded_and_counts_what_it_drops() {
        let b = AuditBook::new(Box::new(forge_cmd::FixedClock(5)));
        for i in 0..(BOOK_CAPACITY + 3) {
            b.record(AuditOrigin::Remote, "", "", "x", i.to_string());
        }
        assert_eq!(b.dropped(), 3);
        assert_eq!(b.len(), (BOOK_CAPACITY + 3) as u64);
        let all = b.query(&AuditQuery::default());
        assert_eq!(all.len(), BOOK_CAPACITY);
        assert_eq!(all[0].detail, "3");
        assert_eq!(all[0].at_ms, 5);
    }
}
