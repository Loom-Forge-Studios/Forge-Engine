//! **Audit log** (`forge.audit_log`, Ch.21 §21.21, Ch.34, Ch.37; DoD M2-54).
//!
//! Every envelope a plugin-hosted session sent (the audit feed its plugin attaches), every
//! security command (plugin and automation grants and revokes, the default automation
//! policy, plugin-set changes, approvals and rejections — as the core applied them, undos
//! included), and every device pairing, unpairing and exposure change, on one timeline,
//! **greppable by session, user and sandbox**: `session:auto-3`, `user:ada`, `sandbox:alice`,
//! `origin:security`, `event:grant`, plus free words. Teammate envelopes have no producer until
//! `forge-collab`; the footer says so.
//!
//! The list is virtualised, and new records arrive as a live feed refreshed at most twice a
//! second while the panel is visible. A refresh reads **only what is new** (each source is
//! read from a cursor) and puts it on top; a new grep rebuilds the list once. The list keeps
//! at most [`MAX_ROWS`], the newest.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use std::collections::VecDeque;

use forge_editor::connect::audit::{AuditCursor, AuditQuery};
use forge_editor::panels::PanelCx;
use forge_ui::widgets::{LabelKind, SearchChanged, SearchField};
use forge_ui::widgets::{RowItem, VirtualTree};
use forge_ui::{LiveFeed, NodeStyle, Role};

use crate::relay::{FeedRelay, FeedTicked};
use crate::ui::{fill, list, set, text};

/// Rows the list keeps (the newest): what the sources keep between them.
pub const MAX_ROWS: usize = 2 * forge_editor::connect::audit::BOOK_CAPACITY;

/// The query the panel shows and where it is in the log (session state).
#[derive(Default)]
struct State {
    query: String,
    /// Bumped when the query changes, so the sync step re-reads.
    asked: u64,
    /// Where the list is in each source.
    cursor: AuditCursor,
    /// The list's keys, oldest first.
    keys: VecDeque<u64>,
    next_key: u64,
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let services = pb.services();
        let space = pb.b.theme_ref().space;
        let root = pb.b.add(
            pb.parent,
            "content",
            NodeStyle::column(space[1]).padding(space[2]).grow(1.0),
            forge_ui::widgets::Container::new(Role::Group).labelled(forge_ui::tr!("Audit log")),
        )?;
        let book_relay =
            pb.b.add(root, "feed", NodeStyle::leaf(), FeedRelay::new())?;
        let feed_relay =
            pb.b.add(root, "extra_feed", NodeStyle::leaf(), FeedRelay::new())?;
        let q = pb.b.signal(String::new());
        let search = pb.b.add(
            root,
            "query",
            NodeStyle::leaf().width(360.0),
            SearchField::new(
                q,
                forge_ui::tr!(
                    "Grep: session:auto-3 user:ada sandbox:x origin:security event:grant words"
                ),
            ),
        )?;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Body)?;
        let records_empty = crate::ui::empty(
            pb,
            root,
            "records_empty",
            forge_ui::tr!(
                "No audit records yet: grants, security decisions and remote sessions are recorded here."
            ),
        )?;
        let records = list(pb, root, "records", forge_ui::tr!("Audit records"), 320.0)?;
        text(
            pb,
            root,
            "footer",
            forge_ui::trf!(
                "Sources: {backend}",
                backend = forge_ui::l10n::tr_str(&services.connect.audit.backend())
            ),
            LabelKind::Small,
        )?;

        let st = Rc::new(RefCell::new(State::default()));
        {
            let st = st.clone();
            pb.on(search, move |act, e: &SearchChanged| {
                let mut s = st.borrow_mut();
                s.query = e.query.clone();
                s.asked += 1;
                act.want_turn();
            });
        }
        for r in [book_relay, feed_relay] {
            pb.on(r, move |act, _: &FeedTicked| act.want_turn());
        }

        let book_live = services.connect.audit_live.clone();
        let feed_live = services.connect.audit.live();
        let audit = services.connect.audit.clone();
        let mut feeds = false;
        let mut seen = (u64::MAX, u64::MAX);
        let mut asked = u64::MAX;
        pb.sync(root, move |s| {
            if !feeds {
                feeds = true;
                s.ui.add_feed(
                    book_relay,
                    LiveFeed {
                        source: book_live.clone() as Arc<dyn forge_ui::LiveSource>,
                        max_hz: crate::LIVE_HZ,
                        self_ui: false,
                    },
                );
                if let Some(src) = &feed_live {
                    s.ui.add_feed(
                        feed_relay,
                        LiveFeed {
                            source: Arc::clone(src),
                            max_hz: crate::LIVE_HZ,
                            self_ui: false,
                        },
                    );
                }
            }
            if s.services.connect.faults.poll_backends() {
                // W2 fault: poll the sources every turn instead of following their feeds.
                s.want_turn();
            }
            let rev = (audit.generation(), st.borrow().asked);
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            let mut st = st.borrow_mut();
            let query = AuditQuery::parse(&st.query);
            // (W2 fault: rebuild on every refresh.)
            let rebuild = st.asked != asked || s.services.connect.faults.audit_full_rebuild();
            asked = st.asked;
            let from = if rebuild {
                AuditCursor::default()
            } else {
                st.cursor
            };
            let (found, cursor) = audit.query_from(&query, from);
            st.cursor = cursor;
            if rebuild {
                fill(s.ui, records, Vec::new());
                st.keys.clear();
            }
            let st = &mut *st;
            // Newest on top: each new record goes in at row 0, oldest-first order kept in
            // `keys`; past MAX_ROWS the oldest rows go.
            VirtualTree::edit(s.ui, records, |t| {
                for r in &found {
                    st.next_key += 1;
                    // The record's stable, greppable line: data, never translated.
                    t.insert(None, 0, st.next_key, RowItem::new(r.line()).verbatim());
                    st.keys.push_back(st.next_key);
                }
                while st.keys.len() > MAX_ROWS {
                    if let Some(k) = st.keys.pop_front() {
                        t.remove(k);
                    }
                }
            });
            crate::ui::show_empty(s.ui, records_empty, st.keys.is_empty());
            let dropped = audit.dropped();
            set(
                s.ui,
                head,
                forge_ui::trf!(
                    "{keys_count} record(s){matching}{dropped_note}",
                    keys_count = st.keys.len(),
                    matching = if query.is_empty() {
                        ""
                    } else {
                        forge_ui::tr!(" match")
                    },
                    dropped_note = if dropped > 0 {
                        forge_ui::trf!(
                            " \u{b7} {dropped} older record(s) let go (the book keeps 65,536)",
                            dropped
                        )
                    } else {
                        String::new()
                    }
                ),
            );
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}
