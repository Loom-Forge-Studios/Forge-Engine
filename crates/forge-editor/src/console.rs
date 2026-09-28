//! The console's model (Ch.21 §21.21 "Console", DoD M2-39): log entries with a level, the
//! subsystem that wrote them, and a **source** the user can click through to — a
//! `SeedPath` (Ch.4 §4.3: "why is this mountain here"), a graph node (a shader or generator
//! error), a refused command, or an entity.
//!
//! * **Grouping.** An entry identical to the newest one (same level, target, message and
//!   source) bumps its repeat count instead of adding a row, so a warning fired every frame
//!   is one row reading "×240", not 240 rows.
//! * **Bounded.** At most [`CONSOLE_CAP`] entries; the oldest go first.
//! * **Threads.** Engine systems log from any thread through a [`LogSender`] (`Send`,
//!   cheap to clone); the shell drains it on the UI thread each loop turn, and the sender
//!   wakes the loop only for the first entry after a drain (zero idle cost, D-5).
//! * **Backend (D-4).** The sink here is the editor's own in-memory log. The engine-wide
//!   trace sink (`forge-trace`, M2-11) is not built; when it lands it feeds this model
//!   through a `LogSender`, and the gate lists it as `UNBUILT` until then.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use forge_cmd::{EntityKey, TxnId};
use forge_ui::fuzzy::ContainsQuery;

/// Entries the console keeps.
pub const CONSOLE_CAP: usize = 10_000;

/// Severity, lowest first.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub const ALL: [LogLevel; 5] = [
        LogLevel::Trace,
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
    ];
    pub fn name(self) -> &'static str {
        match self {
            LogLevel::Trace => "Trace",
            LogLevel::Debug => "Debug",
            LogLevel::Info => "Info",
            LogLevel::Warn => "Warning",
            LogLevel::Error => "Error",
        }
    }
    fn index(self) -> usize {
        self as usize
    }
}

/// What an entry points at (its click-through).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LogSource {
    None,
    /// Generated content: its seed path (`universe/world:3/zone:118/tree:2`).
    SeedPath(String),
    /// A node of a graph (a shader compile error, a generator failure).
    GraphNode {
        graph: String,
        node: u64,
    },
    /// A refused or failed command: what was asked and its stable error code.
    Command {
        what: String,
        code: Option<String>,
        txn: Option<TxnId>,
    },
    /// A project entity.
    Entity(EntityKey),
}

impl LogSource {
    /// A short "go to" text for the row, if the entry has somewhere to go.
    pub fn link_text(&self) -> Option<String> {
        match self {
            LogSource::None => None,
            LogSource::SeedPath(p) => Some(forge_ui::trf!("seed {p}", p)),
            LogSource::GraphNode { graph, node } => {
                Some(forge_ui::trf!("{graph} node {node}", graph, node))
            }
            LogSource::Command { what, code, .. } => Some(match code {
                Some(c) => format!("{what} [{c}]"),
                None => what.clone(),
            }),
            LogSource::Entity(e) => Some(forge_ui::trf!("entity {e}", e)),
        }
    }
}

/// One console row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    /// Stable while the entry exists (the console list's row key).
    pub id: u64,
    pub level: LogLevel,
    /// The subsystem (`forge.asset`, `forge.cmd`, `forge.gen`).
    pub target: String,
    pub message: String,
    pub source: LogSource,
    /// How many identical entries in a row this one stands for.
    pub count: u32,
    /// The source's link text, rendered once when the entry is added (the search and
    /// the row read it; a filter pass allocates nothing).
    pub link: Option<String>,
}

impl LogEntry {
    /// The row text: level (in the UI locale), target, message (as it was logged: a log
    /// line's own text is data filled into the looked-up template), repeat count and link.
    pub fn row_text(&self) -> String {
        let mut s = forge_ui::trf!(
            "{level} \u{b7} {target} \u{2014} {message}",
            level = forge_ui::l10n::tr(self.level.name()),
            target = self.target,
            message = self.message
        );
        if self.count > 1 {
            s.push_str(&format!("  \u{d7}{}", self.count));
        }
        if let Some(l) = &self.link {
            s.push_str(&format!("  \u{2192} {l}"));
        }
        s
    }
}

/// What the console shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleFilter {
    /// Shown levels, indexed by [`LogLevel`] order.
    pub levels: [bool; 5],
    /// Case-insensitive substring over target, message and link text (empty: all),
    /// folded once: testing an entry allocates nothing.
    pub search: ContainsQuery,
}

impl Default for ConsoleFilter {
    fn default() -> Self {
        Self {
            levels: [false, true, true, true, true],
            search: ContainsQuery::default(),
        }
    }
}

impl ConsoleFilter {
    pub fn shows(&self, e: &LogEntry) -> bool {
        if !self.levels[e.level.index()] {
            return false;
        }
        let q = &self.search;
        q.matches(&e.message)
            || q.matches(&e.target)
            || e.link.as_deref().is_some_and(|l| q.matches(l))
    }
}

/// An entry sent from another thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRecord {
    pub level: LogLevel,
    pub target: String,
    pub message: String,
    pub source: LogSource,
}

type WakeFn = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Inbox {
    records: Vec<LogRecord>,
    waker: Option<WakeFn>,
}

/// Logs from any thread into the console (see the module docs).
#[derive(Clone, Default)]
pub struct LogSender {
    inbox: Arc<Mutex<Inbox>>,
}

impl std::fmt::Debug for LogSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogSender").finish_non_exhaustive()
    }
}

impl LogSender {
    pub fn log(&self, level: LogLevel, target: &str, message: &str, source: LogSource) {
        let wake = {
            let mut i = self
                .inbox
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let first = i.records.is_empty();
            i.records.push(LogRecord {
                level,
                target: target.to_string(),
                message: message.to_string(),
                source,
            });
            if first { i.waker.clone() } else { None }
        };
        if let Some(w) = wake {
            w();
        }
    }
    fn take(&self) -> Vec<LogRecord> {
        std::mem::take(
            &mut self
                .inbox
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .records,
        )
    }
    fn set_waker(&self, w: Option<WakeFn>) {
        self.inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .waker = w;
    }
}

/// The console model (see the module docs). Session state: never project state.
#[derive(Debug, Default)]
pub struct ConsoleLog {
    entries: VecDeque<LogEntry>,
    next_id: u64,
    revision: u64,
    sender: LogSender,
    /// An entry the console should scroll to and select ("Details" on a toast).
    focus: Option<u64>,
}

impl ConsoleLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an entry (or bump the newest one's count if identical). Returns its id.
    pub fn push(&mut self, level: LogLevel, target: &str, message: &str, source: LogSource) -> u64 {
        self.revision += 1;
        if let Some(last) = self.entries.back_mut()
            && last.level == level
            && last.target == target
            && last.message == message
            && last.source == source
        {
            last.count = last.count.saturating_add(1);
            return last.id;
        }
        if self.entries.len() >= CONSOLE_CAP {
            self.entries.pop_front();
        }
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push_back(LogEntry {
            id,
            level,
            target: target.to_string(),
            message: message.to_string(),
            link: source.link_text(),
            source,
            count: 1,
        });
        id
    }

    /// Bumped by every change (the console's sync compares it).
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &LogEntry> {
        self.entries.iter()
    }
    pub fn entry(&self, id: u64) -> Option<&LogEntry> {
        // Ids are increasing, so the deque is sorted by id.
        let i = self.entries.binary_search_by_key(&id, |e| e.id).ok()?;
        self.entries.get(i)
    }
    /// The oldest entry's id (`None`: empty).
    pub fn first_id(&self) -> Option<u64> {
        self.entries.front().map(|e| e.id)
    }
    /// The newest entry (the only one a repeat can change).
    pub fn last(&self) -> Option<&LogEntry> {
        self.entries.back()
    }
    /// The id the next new entry gets: every entry ever added has a smaller one.
    pub fn next_id(&self) -> u64 {
        self.next_id
    }
    /// Entries with id `id` or later, oldest first: O(log n) to find the start, so a
    /// console appending what is new costs the new entries only.
    pub fn entries_from(&self, id: u64) -> impl Iterator<Item = &LogEntry> {
        let start = self.entries.partition_point(|e| e.id < id);
        self.entries.range(start..)
    }
    /// The entries `f` shows, oldest first.
    pub fn filtered<'a>(&'a self, f: &'a ConsoleFilter) -> impl Iterator<Item = &'a LogEntry> {
        self.entries.iter().filter(move |e| f.shows(e))
    }
    /// Count per level (the filter buttons' badges).
    pub fn counts(&self) -> [u64; 5] {
        let mut c = [0u64; 5];
        for e in &self.entries {
            c[e.level.index()] += u64::from(e.count);
        }
        c
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.focus = None;
        self.revision += 1;
    }

    /// A sender for other threads.
    pub fn sender(&self) -> LogSender {
        self.sender.clone()
    }
    /// Install the loop waker the sender calls (the shell does this once).
    pub fn set_waker(&self, w: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.sender.set_waker(w);
    }
    /// Move what other threads sent into the log. Returns how many arrived.
    pub fn drain(&mut self) -> usize {
        let recs = self.sender.take();
        let n = recs.len();
        for r in recs {
            self.push(r.level, &r.target, &r.message, r.source);
        }
        n
    }

    /// Ask the console to reveal entry `id`.
    pub fn focus(&mut self, id: u64) {
        self.focus = Some(id);
        self.revision += 1;
    }
    /// The entry to reveal, once.
    pub fn take_focus(&mut self) -> Option<u64> {
        self.focus.take()
    }
    pub fn pending_focus(&self) -> Option<u64> {
        self.focus
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_entries_group_and_the_log_is_bounded() {
        let mut l = ConsoleLog::new();
        let a = l.push(LogLevel::Warn, "t", "m", LogSource::None);
        let b = l.push(LogLevel::Warn, "t", "m", LogSource::None);
        assert_eq!(a, b);
        assert_eq!(l.len(), 1);
        assert_eq!(l.entries().next().map(|e| e.count), Some(2));
        l.push(LogLevel::Warn, "t", "other", LogSource::None);
        l.push(LogLevel::Warn, "t", "m", LogSource::None);
        assert_eq!(l.len(), 3, "only consecutive repeats group");
        for i in 0..CONSOLE_CAP + 5 {
            l.push(LogLevel::Info, "t", &format!("{i}"), LogSource::None);
        }
        assert_eq!(l.len(), CONSOLE_CAP);
        let first = l.entries().next().map(|e| e.id).unwrap_or(0);
        assert!(l.entry(first).is_some());
    }

    #[test]
    fn filters_by_level_and_search_including_the_link() {
        let mut l = ConsoleLog::new();
        l.push(LogLevel::Debug, "gen", "noise", LogSource::None);
        l.push(
            LogLevel::Error,
            "gen",
            "bad slope",
            LogSource::SeedPath("universe/world:3".into()),
        );
        let mut f = ConsoleFilter::default();
        assert_eq!(l.filtered(&f).count(), 2);
        f.levels[LogLevel::Debug as usize] = false;
        assert_eq!(l.filtered(&f).count(), 1);
        f.levels[LogLevel::Debug as usize] = true;
        f.search = ContainsQuery::new("WORLD:3");
        let hits: Vec<_> = l.filtered(&f).map(|e| e.message.clone()).collect();
        assert_eq!(hits, vec!["bad slope".to_string()]);
    }

    #[test]
    fn the_sender_wakes_once_per_drain() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let mut l = ConsoleLog::new();
        let wakes = Arc::new(AtomicU32::new(0));
        let w = wakes.clone();
        l.set_waker(Some(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        })));
        let s = l.sender();
        let t = std::thread::spawn(move || {
            for i in 0..10 {
                s.log(LogLevel::Info, "worker", &format!("{i}"), LogSource::None);
            }
        });
        let _ = t.join();
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(l.drain(), 10);
        l.sender()
            .log(LogLevel::Info, "w", "again", LogSource::None);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }
}
