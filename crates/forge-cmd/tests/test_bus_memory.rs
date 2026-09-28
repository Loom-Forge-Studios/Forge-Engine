//! The bus's memory stays bounded in a long editor session (owner rule 2): a drag of any
//! length costs one change slot per property it touched, retired transactions keep no
//! record, duplicate refusal uses a fixed window, and gesture frames fold into one audit
//! entry per target. Pinned as: footprint <= history cap x touched properties, and the same
//! footprint for a 3 000-frame drag as for a 10 000-frame one.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use forge_cmd::{
    AuditAction, AuditEntry, Bus, Change, Clock, CommandSink, EditorCommand, EntityKey, Footprint,
    Issuer, REPLAY_WINDOW, RETIRED_WINDOW, StreamItem, Subscription, TxnState, Value,
};

fn human() -> Issuer {
    Issuer::Human { user: "ada".into() }
}

fn set(entity: EntityKey, path: &str, v: f64) -> EditorCommand {
    EditorCommand::SetProperty {
        entity,
        path: path.into(),
        value: Value::Float(v),
    }
}

fn spawn(bus: &mut Bus) -> (EntityKey, forge_cmd::CommandEnvelope) {
    let e = bus.envelope(
        human(),
        EditorCommand::Spawn {
            name: "Cube".into(),
            parent: None,
        },
    );
    let sent = e.clone();
    match bus.apply(e).expect("spawn").diff.changes.first() {
        Some(Change::Created { entity, .. }) => (*entity, sent),
        other => panic!("spawn produced {other:?}"),
    }
}

const HISTORY: usize = 10;
const TOUCHED: usize = 2;
const EDITS: usize = 100;

/// Spawn, one `frames`-long drag over two properties, then `EDITS` single edits.
fn long_session(frames: u32) -> (Bus, forge_cmd::TxnId, forge_cmd::CommandEnvelope) {
    let mut bus = Bus::new().with_max_history(HISTORY);
    let (cube, first) = spawn(&mut bus);
    let drag = bus.begin("Drag position", human());
    for f in 1..=frames {
        for path in ["x", "y"] {
            let e = bus.envelope_in(drag, human(), set(cube, path, f64::from(f)));
            bus.apply(e).expect("frame");
        }
        if f == frames / 2 {
            // Mid-drag: one slot per touched property, one fold entry per target.
            let rec = bus.transaction(drag).expect("open");
            assert_eq!(rec.changes().count(), TOUCHED);
            assert_eq!(rec.commands().count, u64::from(f) * 2);
            let fp = bus.footprint();
            assert_eq!(
                fp.change_slots,
                1 + TOUCHED,
                "the spawn's slot + the drag's"
            );
            assert_eq!(fp.audit_fold_index, TOUCHED);
        }
    }
    bus.commit(drag).expect("commit");
    for i in 0..EDITS {
        let path = if i % 2 == 0 { "z" } else { "w" };
        let e = bus.envelope(human(), set(cube, path, i as f64));
        bus.apply(e).expect("edit");
    }
    (bus, drag, first)
}

#[test]
fn a_long_session_holds_history_cap_times_touched_properties() {
    let frames = 10_000;
    let (mut bus, drag, first) = long_session(frames);
    let fp = bus.footprint();

    // Undo memory: only the live history, each entry one slot per property it touched.
    assert!(fp.transactions <= HISTORY, "{fp:?}");
    assert!(fp.change_slots <= HISTORY * TOUCHED, "{fp:?}");
    assert_eq!(bus.history().count(), HISTORY);
    // Retired transactions keep no record, only a bounded tombstone.
    assert!(
        bus.transaction(drag).is_none(),
        "the expired drag's record is gone"
    );
    assert_eq!(bus.txn_state(drag), Some(TxnState::Expired));
    assert!(fp.tombstones <= RETIRED_WINDOW);
    // Duplicate refusal: a fixed window, not every id ever sent (20 000 + 101 commands).
    assert!(fp.replay_ids <= REPLAY_WINDOW, "{fp:?}");
    assert_eq!(fp.audit_fold_index, 0, "no gesture is open");

    // Audit: spawn + begin + one folded line per dragged property + commit + the edits.
    assert_eq!(fp.audit_entries, 1 + 1 + TOUCHED + 1 + EDITS, "{fp:?}");
    let folded: Vec<_> = bus.audit().iter().filter(|a| a.merged > 1).collect();
    assert_eq!(folded.len(), TOUCHED);
    for a in &folded {
        assert_eq!(a.merged, u64::from(frames));
        assert_eq!(a.txn, drag);
        assert_eq!(a.issuer, human());
        assert!(a.first_command < a.command);
        match &a.action {
            AuditAction::Command(EditorCommand::SetProperty { value, .. }) => {
                assert_eq!(*value, Value::Float(f64::from(frames)), "the latest frame");
            }
            other => panic!("unexpected folded action {other:?}"),
        }
    }

    // The bounds do not weaken the refusals: an ancient resend, a late frame into the
    // expired drag, and an undo of it are all still refused with their stable codes.
    assert_eq!(
        bus.apply(first).expect_err("stale").code().as_str(),
        "CMD-0013"
    );
    let cube = EntityKey(0);
    let late = bus.envelope_in(drag, human(), set(cube, "x", 1.0));
    assert_eq!(
        bus.apply(late).expect_err("expired").code().as_str(),
        "CMD-0006"
    );
    assert_eq!(
        bus.undo(drag).expect_err("expired").code().as_str(),
        "CMD-0006"
    );
}

#[test]
fn the_footprint_does_not_depend_on_drag_length() {
    // Both drags overflow the fixed replay window, so every field must match exactly.
    let short: Footprint = long_session(3_000).0.footprint();
    let long: Footprint = long_session(10_000).0.footprint();
    assert_eq!(short, long, "memory is per property touched, not per frame");
}

#[test]
fn the_undrained_audit_is_capped_and_every_drop_is_counted() {
    let mut bus = Bus::new().with_max_audit(100);
    let (cube, _) = spawn(&mut bus);
    let edits = 1_000_u64;
    for i in 0..edits {
        let e = bus.envelope(human(), set(cube, "x", i as f64));
        bus.apply(e).expect("edit");
    }
    let kept = bus.audit().len() as u64;
    assert!(kept <= 100);
    assert_eq!(
        kept + bus.audit_dropped(),
        1 + edits,
        "nothing vanishes uncounted"
    );
    // The newest entries are the ones kept, contiguous.
    let last = bus.audit().last().expect("kept");
    assert_eq!(last.seq, edits);
    assert!(bus.audit().windows(2).all(|w| w[1].seq == w[0].seq + 1));
}

#[test]
fn only_the_gesture_owner_folds_and_a_drain_starts_a_fresh_line() {
    let mut bus = Bus::new();
    let (cube, _) = spawn(&mut bus);
    let drag = bus.begin("Drag x", human());
    for x in 1..=5 {
        let e = bus.envelope_in(drag, human(), set(cube, "x", f64::from(x)));
        bus.apply(e).expect("frame");
    }
    // Someone else sending into the gesture is refused, and each attempt gets its own line.
    let other = Issuer::Automation {
        session: "s".into(),
        tool: "t".into(),
    };
    for _ in 0..3 {
        let e = bus.envelope_in(drag, other.clone(), set(cube, "x", 0.0));
        assert_eq!(
            bus.apply(e).expect_err("not theirs").code().as_str(),
            "CMD-0012"
        );
    }
    let lines = bus.drain_audit();
    let frames: Vec<_> = lines
        .iter()
        .filter(|a| matches!(a.action, AuditAction::Command(_)) && a.txn == drag)
        .collect();
    assert_eq!(
        frames.len(),
        1 + 3,
        "one folded owner line + three refusals"
    );
    assert_eq!(frames[0].merged, 5);
    assert!(
        frames[1..]
            .iter()
            .all(|a| a.merged == 1 && a.issuer == other)
    );

    // After a drain the folded entry is gone; the next frame starts a new line.
    let e = bus.envelope_in(drag, human(), set(cube, "x", 6.0));
    bus.apply(e).expect("frame");
    let e = bus.envelope_in(drag, human(), set(cube, "x", 7.0));
    bus.apply(e).expect("frame");
    assert_eq!(bus.audit().len(), 1);
    assert_eq!(bus.audit()[0].merged, 2);
    bus.commit(drag).expect("commit");
    assert_eq!(bus.footprint().audit_fold_index, 0);
}

#[test]
fn old_tombstones_fall_back_to_a_watermark_not_to_unknown() {
    let mut bus = Bus::new();
    let (cube, _) = spawn(&mut bus);
    let mut first = None;
    for i in 0..=RETIRED_WINDOW {
        let t = bus.begin("Esc", human());
        first.get_or_insert(t);
        let e = bus.envelope_in(t, human(), set(cube, "x", i as f64));
        bus.apply(e).expect("frame");
        bus.cancel(t).expect("cancel");
    }
    let first = first.expect("ran");
    let fp = bus.footprint();
    assert!(fp.tombstones <= RETIRED_WINDOW, "{fp:?}");
    assert_eq!(fp.transactions, 1, "only the spawn is live");
    assert_eq!(bus.txn_state(first), Some(TxnState::Expired));
    assert_eq!(
        bus.undo(first).expect_err("gone").code().as_str(),
        "CMD-0006"
    );
    let late = bus.envelope_in(first, human(), set(cube, "x", 1.0));
    assert_eq!(
        bus.apply(late).expect_err("gone").code().as_str(),
        "CMD-0006"
    );
}

fn spawn_named(bus: &mut Bus, name: &str) -> EntityKey {
    let e = bus.envelope(
        human(),
        EditorCommand::Spawn {
            name: name.into(),
            parent: None,
        },
    );
    match bus.apply(e).expect("spawn").diff.changes.first() {
        Some(Change::Created { entity, .. }) => *entity,
        other => panic!("spawn produced {other:?}"),
    }
}

// ---- the stream: a stalled subscriber is bounded and told what it missed -------------------

type Props = BTreeMap<(EntityKey, String), Value>;

/// A mirror of entity properties built only from the stream, as the split editor and a
/// panel build theirs. `honour_gaps: false` is the positive control: a mirror that ignores
/// the gap marker silently diverges.
struct Mirror {
    props: Props,
    /// Events with `seq` below this are already in the last snapshot.
    floor: u64,
    honour_gaps: bool,
}

impl Mirror {
    fn snapshot(bus: &Bus, honour_gaps: bool) -> Self {
        let mut m = Self {
            props: Props::new(),
            floor: 0,
            honour_gaps,
        };
        m.resync(bus);
        m
    }

    fn resync(&mut self, bus: &Bus) {
        self.props.clear();
        for (k, e) in bus.project().entities() {
            for (p, v) in e.properties() {
                self.props.insert((k, p.to_string()), v.clone());
            }
        }
        self.floor = bus.next_seq();
    }

    fn pump(&mut self, bus: &Bus, feed: &Subscription) {
        let got = feed.drain();
        if got.gap.is_some() && self.honour_gaps {
            self.resync(bus);
        }
        for a in got.iter().filter(|a| a.seq >= self.floor) {
            for c in &a.diff.changes {
                if let Change::Property {
                    entity,
                    path,
                    after,
                    ..
                } = c
                {
                    let k = (*entity, path.to_string());
                    match after {
                        Some(v) => self.props.insert(k, v.clone()),
                        None => self.props.remove(&k),
                    };
                }
            }
        }
    }
}

fn truth(bus: &Bus) -> Props {
    Mirror::snapshot(bus, true).props
}

#[test]
fn a_stalled_subscriber_is_bounded_and_resyncs_from_a_gap() {
    const CAP: usize = 16;
    let mut bus = Bus::new();
    let cube = spawn_named(&mut bus, "Cube");
    let feed = bus.subscribe_with_capacity(CAP);
    let naive_feed = bus.subscribe_with_capacity(CAP);
    let mut good = Mirror::snapshot(&bus, true);
    let mut naive = Mirror::snapshot(&bus, false);

    // A burst the subscribers do not pump: 10 000 edits.
    let first_seq = bus.next_seq();
    let edits = 10_000_u64;
    for i in 0..edits {
        let path = if i % 2 == 0 { "x" } else { "y" };
        let e = bus.envelope(human(), set(cube, path, i as f64));
        bus.apply(e).expect("edit");
        assert!(feed.pending() <= CAP, "a stalled subscriber must not grow");
    }
    assert_eq!(feed.pending(), 0, "the queue coalesced into one gap marker");
    assert_eq!(bus.footprint().subscribers, 2);

    // The gap says exactly what was missed, once.
    let d = feed.drain();
    let gap = d.gap.expect("a gap marker");
    assert!(d.events.is_empty());
    assert_eq!(gap.first_missed, first_seq);
    assert_eq!(gap.last_missed, first_seq + edits - 1);
    assert_eq!(gap.missed, edits);
    assert!(feed.drain().is_empty(), "the gap is delivered once");

    // A mirror that honours the gap resyncs, then follows live events exactly.
    good.resync(&bus);
    for i in 0..5 {
        let e = bus.envelope(human(), set(cube, "z", f64::from(i)));
        bus.apply(e).expect("edit");
    }
    good.pump(&bus, &feed);
    assert_eq!(good.props, truth(&bus));

    // Positive control: ignoring the gap leaves a mirror that silently disagrees.
    naive.pump(&bus, &naive_feed);
    assert_ne!(naive.props, truth(&bus), "a mirror ignoring gaps diverges");

    // try_next delivers events one at a time once the gap is consumed.
    let e = bus.envelope(human(), set(cube, "w", 1.0));
    bus.apply(e).expect("edit");
    assert!(matches!(feed.try_next(), Some(StreamItem::Applied(_))));
    assert!(feed.try_next().is_none());

    // A dropped subscription is pruned on the next event.
    drop(naive_feed);
    let e = bus.envelope(human(), set(cube, "w", 2.0));
    bus.apply(e).expect("edit");
    assert_eq!(bus.footprint().subscribers, 1);
}

#[test]
fn try_next_delivers_a_pending_gap_before_anything_else() {
    let mut bus = Bus::new();
    let cube = spawn_named(&mut bus, "Cube");
    let feed = bus.subscribe_with_capacity(2);
    for i in 0..3 {
        let e = bus.envelope(human(), set(cube, "x", f64::from(i)));
        bus.apply(e).expect("edit");
    }
    match feed.try_next() {
        Some(StreamItem::Gap(g)) => assert_eq!(g.missed, 3),
        other => panic!("expected the gap first, got {other:?}"),
    }
    assert!(feed.try_next().is_none());
}

#[test]
fn a_subscriber_that_keeps_up_never_sees_a_gap() {
    let mut bus = Bus::new();
    let cube = spawn_named(&mut bus, "Cube");
    let feed = bus.subscribe_with_capacity(4);
    let mut m = Mirror::snapshot(&bus, true);
    for i in 0..1_000 {
        let e = bus.envelope(human(), set(cube, "x", f64::from(i)));
        bus.apply(e).expect("edit");
        let before = m.props.len();
        m.pump(&bus, &feed);
        assert!(m.props.len() >= before);
    }
    assert_eq!(m.props, truth(&bus));
}

// ---- audit folding never reorders the trail -------------------------------------------------

/// A clock that advances 1 ms per reading, so every audit timestamp is distinct and the
/// trail's order can be checked against time.
struct Ticking(AtomicU64);

impl Clock for Ticking {
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

/// Entries of another transaction or issuer whose time falls strictly inside a folded
/// entry's span: each is an event the fold moved a frame across.
fn reorderings(trail: &[AuditEntry]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for f in trail.iter().filter(|a| a.merged > 1) {
        for o in trail {
            let foreign = o.txn != f.txn || o.issuer != f.issuer;
            if foreign && o.at_ms > f.at_ms && o.at_ms < f.last_at_ms {
                out.push((f.seq, o.seq));
            }
        }
    }
    out
}

#[test]
fn audit_folding_never_moves_a_frame_across_another_entry() {
    let mut bus = Bus::with_clock(Box::new(Ticking(AtomicU64::new(0))));
    let cube = spawn_named(&mut bus, "Cube");
    let other = spawn_named(&mut bus, "Other");
    let auto = Issuer::Automation {
        session: "s".into(),
        tool: "t".into(),
    };
    let drag = bus.begin("Drag", human());
    let frame = |bus: &mut Bus, v: f64| {
        for p in ["x", "y"] {
            let e = bus.envelope_in(drag, human(), set(cube, p, v));
            bus.apply(e).expect("frame");
        }
    };
    for f in 1..=10 {
        frame(&mut bus, f64::from(f));
    }
    // Interruptions: an automation session's refused frame into the drag, then an automation edit
    // elsewhere.
    let e = bus.envelope_in(drag, auto.clone(), set(cube, "x", 0.0));
    assert!(bus.apply(e).is_err());
    for f in 11..=20 {
        frame(&mut bus, f64::from(f));
    }
    let e = bus.envelope(auto.clone(), set(other, "x", 1.0));
    bus.apply(e).expect("automation edit");
    for f in 21..=30 {
        frame(&mut bus, f64::from(f));
    }
    bus.commit(drag).expect("commit");

    let trail = bus.audit();
    assert!(reorderings(trail).is_empty(), "{:?}", reorderings(trail));
    assert!(
        trail
            .windows(2)
            .all(|w| w[0].seq < w[1].seq && w[0].at_ms < w[1].at_ms)
    );
    // Each run between interruptions folds to one line per property: 3 runs x 2 properties.
    let folded: Vec<u64> = trail
        .iter()
        .filter(|a| {
            a.txn == drag && a.issuer == human() && matches!(a.action, AuditAction::Command(_))
        })
        .map(|a| a.merged)
        .collect();
    assert_eq!(folded, vec![10, 10, 10, 10, 10, 10]);
    assert!(
        trail
            .iter()
            .filter(|a| a.merged > 1)
            .all(|a| a.last_at_ms > a.at_ms)
    );

    // Positive control for the checker: a first-run line that also swallowed a frame from
    // after the session's refusal (what folding into any undrained line did) is flagged.
    let mut bad = trail.to_vec();
    let automation_line = bad
        .iter()
        .position(|a| a.issuer == auto)
        .expect("the refusal is in the trail");
    let late = bad[automation_line + 1].last_at_ms;
    if let Some(first_run) = bad.iter_mut().find(|a| a.merged > 1) {
        first_run.last_at_ms = late;
    }
    assert!(
        !reorderings(&bad).is_empty(),
        "the checker catches a reordering fold"
    );
}

#[test]
fn draining_the_audit_keeps_its_buffers() {
    let mut bus = Bus::new();
    let cube = spawn_named(&mut bus, "Cube");
    for i in 0..500 {
        let e = bus.envelope(human(), set(cube, "x", f64::from(i)));
        bus.apply(e).expect("edit");
    }
    let cap = bus.audit_capacity();
    assert!(cap >= 501);
    let mut sink = Vec::new();
    bus.drain_audit_into(&mut sink);
    assert_eq!(sink.len(), 501);
    assert_eq!(
        bus.audit_capacity(),
        cap,
        "drain_audit_into keeps the bus's buffer"
    );
    let out_cap = sink.capacity();
    sink.clear();
    for i in 0..100 {
        let e = bus.envelope(human(), set(cube, "x", f64::from(i) + 0.5));
        bus.apply(e).expect("edit");
    }
    bus.drain_audit_into(&mut sink);
    assert_eq!(sink.len(), 100);
    assert_eq!(sink.capacity(), out_cap, "and reuses the caller's");
    assert!(bus.drain_audit().is_empty());
    assert_eq!(bus.audit_capacity(), cap, "drain_audit keeps it too");
}
