//! The **Console** (`forge.console`, Ch.21 §21.21, DoD M2-39): the editor log with level
//! filters, search, grouping of repeated lines (the model groups them: "×240"), and
//! click-through to where an entry came from:
//!
//! * a `SeedPath` opens the panel a plugin declared for seed paths at that node (a
//!   [`SeedPathPanel`]; Ch.4 §4.3), when one did;
//! * a graph node opens the graph editor at that node (a shader or generator error);
//! * a refused command shows the command and its stable error code, and a command with a
//!   transaction opens the undo history at it;
//! * an entity selects it in the hierarchy.
//!
//! A refused command's toast has "Details", which opens the console at its entry. The list
//! is virtualised and follows the log incrementally: a new line costs one row (two at the
//! 10,000-entry cap, where the oldest is trimmed from the front). The log is
//! session state: clearing it changes nothing in the project.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use forge_editor::console::{ConsoleFilter, ConsoleLog, LogLevel, LogSource};
use forge_editor::panels::PanelCx;
use forge_editor::services::EditorServices;
use forge_editor::session::{Reveal, SeedPathPanel, SessionState, ShellRequest};
use forge_ui::dock::PanelId;
use forge_ui::widgets::{
    Button, Checkbox, Container, Label, LabelKind, Pressed, RowActivated, RowItem, SearchChanged,
    SearchField, SelectionChanged, SignalChanged, SignalRelay, VirtualTree,
};
use forge_ui::{NodeStyle, Role, Signal, Ui, WidgetId};

/// The levels with a filter box, in order (Trace is off by default).
const LEVELS: [(LogLevel, &str); 5] = [
    (LogLevel::Trace, "trace"),
    (LogLevel::Debug, "debug"),
    (LogLevel::Info, "info"),
    (LogLevel::Warn, "warn"),
    (LogLevel::Error, "error"),
];

struct View {
    list: WidgetId,
    detail: Signal<String>,
    filter: ConsoleFilter,
    /// The ids of the rows shown, oldest first.
    shown: VecDeque<u64>,
    /// The empty state, shown while no row is (M2-70).
    empty: WidgetId,
    /// Every log entry with a smaller id has been looked at (shown or filtered out).
    cursor: u64,
    seen_rev: u64,
    seen_reveal: u64,
    selected: Option<u64>,
    /// Positive control for `console_follows_in_constant_work`: rebuild every row on each
    /// log change.
    fault_rebuild: bool,
}

impl View {
    /// Bring the rows in line with the log in work proportional to what changed: drop
    /// rows whose entries left the log (the oldest go first at the cap, so they are
    /// trimmed from the front), relabel the newest row if a repeat bumped its count, and
    /// append the entries past the cursor that the filter shows. A new line at the cap
    /// therefore costs two rows, not 10,000. `force` (a filter change) rebuilds.
    fn follow(&mut self, ui: &mut Ui, log: &ConsoleLog, force: bool) {
        if !force && log.revision() == self.seen_rev {
            return;
        }
        self.seen_rev = log.revision();
        let (shown, filter, cursor) = (&mut self.shown, &self.filter, self.cursor);
        if force || self.fault_rebuild {
            shown.clear();
            let rows: Vec<(u64, RowItem)> = log
                .filtered(filter)
                .map(|e| {
                    shown.push_back(e.id);
                    (e.id, RowItem::new(e.row_text()))
                })
                .collect();
            VirtualTree::edit(ui, self.list, |t| {
                t.clear();
                t.extend(rows);
            });
        } else {
            VirtualTree::edit(ui, self.list, |t| {
                match log.first_id() {
                    // Cleared, or every row's entry is gone: start over cheaply.
                    None => {
                        shown.clear();
                        t.clear();
                    }
                    Some(_) if shown.back().is_some_and(|b| log.entry(*b).is_none()) => {
                        shown.clear();
                        t.clear();
                    }
                    Some(first) => {
                        while let Some(&id) = shown.front() {
                            if id >= first {
                                break;
                            }
                            shown.pop_front();
                            t.remove(id);
                        }
                    }
                }
                if let (Some(&id), Some(last)) = (shown.back(), log.last())
                    && last.id == id
                {
                    let text = last.row_text();
                    if t.label_of(id) != Some(text.as_str()) {
                        t.set_label(id, &text);
                    }
                }
                let new: Vec<(u64, RowItem)> = log
                    .entries_from(cursor)
                    .filter(|e| filter.shows(e))
                    .map(|e| {
                        shown.push_back(e.id);
                        (e.id, RowItem::new(e.row_text()))
                    })
                    .collect();
                t.extend(new);
            });
        }
        self.cursor = log.next_id();
        let _ = ui.set_hidden(self.empty, !self.shown.is_empty());
    }

    fn reveal(&mut self, ui: &mut Ui, log: &ConsoleLog, id: u64) {
        if !self.shown.contains(&id) {
            // Show everything so the entry is on the list.
            self.filter = ConsoleFilter {
                levels: [true; 5],
                search: forge_ui::fuzzy::ContainsQuery::default(),
            };
            self.follow(ui, log, true);
        }
        VirtualTree::edit(ui, self.list, |t| {
            t.select(&[id]);
            if let Some(r) = t.row_of(id) {
                t.scroll_to_row(r);
            }
        });
        self.select(ui, log, Some(id));
    }

    fn select(&mut self, ui: &mut Ui, log: &ConsoleLog, id: Option<u64>) {
        self.selected = id;
        let text = id.and_then(|i| log.entry(i)).map_or(String::new(), |e| {
            let src = e
                .source
                .link_text()
                .map_or(String::new(), |l| forge_ui::trf!("\nSource: {l}", l));
            forge_ui::trf!(
                "{level} \u{b7} {target}\n{message}{src}",
                level = forge_ui::l10n::tr(e.level.name()),
                target = e.target,
                message = e.message,
                src
            )
        });
        self.detail.set(ui.rt_mut(), text);
    }
}

/// Go where an entry came from.
fn go_to_source(session: &mut SessionState, services: &EditorServices, src: &LogSource) {
    match src {
        LogSource::SeedPath(p) => {
            // No declared seed-path panel: the path is shown, and there is nowhere to go.
            if let Some(target) = SeedPathPanel::of(services) {
                target.reveal(session, p.clone());
            }
        }
        LogSource::GraphNode { graph, node } => session.reveal(
            Reveal::GraphNode {
                graph: graph.clone(),
                node: *node,
            },
            "forge.graph",
        ),
        LogSource::Command { txn: Some(t), .. } => {
            session.reveal(Reveal::Txn(*t), "forge.undo_history")
        }
        LogSource::Entity(e) => {
            session.set_selection(vec![*e]);
            session.request(ShellRequest::OpenPanel(PanelId::new("forge.hierarchy")));
        }
        LogSource::Command { txn: None, .. } | LogSource::None => {}
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let services = pb.services();
        let log = Rc::clone(&services.log);
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space[1]).padding(space[1]),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Console filters")),
        )?;
        let filter0 = ConsoleFilter::default();
        let mut level_sigs = Vec::new();
        for (l, key) in LEVELS.iter() {
            let sig = pb.b.signal(filter0.levels[*l as usize]);
            pb.b.add(bar, *key, NodeStyle::leaf(), Checkbox::new(sig, forge_ui::l10n::tr(l.name())))?;
            let relay = pb.b.add(
                bar,
                forge_ui::Key::Str(format!("{key}.relay").into()),
                NodeStyle::leaf(),
                SignalRelay::new(sig.any()),
            )?;
            level_sigs.push((*l, sig, relay));
        }
        let query = pb.b.signal(String::new());
        let search = pb.b.add(bar, "search", NodeStyle::leaf().grow(1.0), SearchField::new(query, forge_ui::tr!("Search the console")))?;
        let clear = pb.b.add(bar, "clear", NodeStyle::leaf(), Button::new(forge_ui::tr!("Clear")))?;
        let go = pb.b.add(bar, "go", NodeStyle::leaf(), Button::new(forge_ui::tr!("Go to source")))?;
        let mut t = VirtualTree::list(forge_ui::tr!("Console")).read_only().single_select();
        let (shown, cursor) = {
            let lg = log.borrow();
            let mut shown = VecDeque::new();
            t.extend(lg.filtered(&filter0).map(|e| {
                shown.push_back(e.id);
                (e.id, RowItem::new(e.row_text()))
            }));
            (shown, lg.next_id())
        };
        // M2-70: no line to show (none yet, or none the filters let through).
        let empty = pb.b.add(
            pb.parent,
            "empty",
            NodeStyle::leaf(),
            forge_ui::widgets::EmptyState::new(forge_ui::tr!(
                "No messages to show: none yet, or none that the filters and the search let through."
            )),
        )?;
        pb.b.hide(empty, !shown.is_empty());
        let list = pb.b.add(pb.parent, "list", NodeStyle::leaf().grow(1.0), t)?;
        let detail = pb.b.signal(String::new());
        pb.b.add(pb.parent, "detail", NodeStyle::leaf().padding(space[1]), Label::new(detail).kind(LabelKind::Mono).wrapping())?;
        pb.b.add(
            pb.parent,
            "backend",
            NodeStyle::leaf().padding(space[1]),
            Label::new(forge_ui::tr!("Editor log (in memory, D-4) \u{2014} the engine trace sink forge-trace (M2-11) is not built yet."))
                .kind(LabelKind::Small)
                .wrapping(),
        )?;
        let view = Rc::new(RefCell::new(View {
            list,
            detail,
            filter: filter0,
            shown,
            empty,
            cursor,
            seen_rev: log.borrow().revision(),
            seen_reveal: 0,
            selected: None,
            fault_rebuild: services.faults.console_rebuild_every_line(),
        }));

        for (l, sig, relay) in level_sigs {
            let (v, lg) = (view.clone(), log.clone());
            pb.on(relay, move |act, _: &SignalChanged| {
                let on = sig.get(act.ui.rt());
                let mut v = v.borrow_mut();
                if v.filter.levels[l as usize] != on {
                    v.filter.levels[l as usize] = on;
                    v.follow(act.ui, &lg.borrow(), true);
                }
            });
        }
        let (v, lg) = (view.clone(), log.clone());
        pb.on(search, move |act, e: &SearchChanged| {
            let mut v = v.borrow_mut();
            let q = forge_ui::fuzzy::ContainsQuery::new(&e.query);
            if q == v.filter.search {
                return;
            }
            v.filter.search = q;
            v.follow(act.ui, &lg.borrow(), true);
        });
        pb.on(clear, |act, _: &Pressed| act.services.log.borrow_mut().clear());
        pb.on_op(list, "console.clear", |act| act.services.log.borrow_mut().clear());
        let (v, lg) = (view.clone(), log.clone());
        pb.on(list, move |act, e: &SelectionChanged| {
            v.borrow_mut().select(act.ui, &lg.borrow(), e.keys.first().copied());
        });
        let lg = log.clone();
        pb.on(list, move |act, e: &RowActivated| {
            let src = lg.borrow().entry(e.key).map(|e| e.source.clone());
            if let Some(s) = src {
                go_to_source(act.session, act.services, &s);
            }
        });
        let (v, lg) = (view.clone(), log.clone());
        pb.on(go, move |act, _: &Pressed| {
            let id = v.borrow().selected;
            let src = id.and_then(|i| lg.borrow().entry(i).map(|e| e.source.clone()));
            if let Some(s) = src {
                go_to_source(act.session, act.services, &s);
            }
        });
        let (v, lg) = (view, log);
        pb.sync(list, move |s| {
            let mut v = v.borrow_mut();
            let log = lg.borrow();
            v.follow(s.ui, &log, false);
            if let Some((seq, Reveal::LogEntry(id))) = s.session.pending_reveal()
                && *seq != v.seen_reveal
            {
                v.seen_reveal = *seq;
                let id = *id;
                v.reveal(s.ui, &log, id);
            }
            Ok(())
        });
        Ok(())
    });
}
