//! The Perfetto sink: a recording encoded as Perfetto's native protobuf trace format
//! (`perfetto.protos.Trace`, the `.pftrace` files <https://ui.perfetto.dev> and
//! `trace_processor` open), written by hand — the handful of messages below need no protobuf
//! dependency.
//!
//! Layout: one trusted packet sequence. First the track descriptors — the process, one
//! thread track per recording thread, one counter track per counter name — then every event
//! in timestamp order: each thread's zones as properly nested `SLICE_BEGIN`/`SLICE_END`
//! pairs, counter samples as `COUNTER` events (`double_counter_value`), and instants (frame
//! marks, budget overruns, play controls) as `INSTANT` events.
//!
//! Field numbers (perfetto/protos/perfetto/trace): `Trace.packet = 1`; `TracePacket`:
//! `timestamp = 8`, `trusted_packet_sequence_id = 10`, `track_event = 11`,
//! `sequence_flags = 13`, `track_descriptor = 60`; `TrackDescriptor`: `uuid = 1`, `name = 2`,
//! `process = 3`, `thread = 4`, `parent_uuid = 5`, `counter = 8`; `ProcessDescriptor`:
//! `pid = 1`, `process_name = 6`; `ThreadDescriptor`: `pid = 1`, `tid = 2`,
//! `thread_name = 5`; `TrackEvent`: `type = 9`, `track_uuid = 11`, `name = 23`,
//! `double_counter_value = 44`.
//!
//! [`read`] decodes the same subset back (the tests read every trace they write, and a tool
//! can summarise a trace without the Perfetto UI).

use std::collections::BTreeMap;

use crate::Recording;

/// One closed zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Slice {
    pub(crate) tid: u32,
    pub(crate) name: &'static str,
    pub(crate) start: u64,
    pub(crate) end: u64,
}

/// One counter sample.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CounterSample {
    pub(crate) name: &'static str,
    pub(crate) ts: u64,
    pub(crate) value: f64,
}

/// One instant event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Instant {
    pub(crate) tid: u32,
    pub(crate) name: String,
    pub(crate) ts: u64,
}

const SEQ: u64 = 1;
const PROCESS_UUID: u64 = 1;
const THREAD_UUID: u64 = 0x1_0000;
const COUNTER_UUID: u64 = 0x100_0000;
/// Synthetic thread ids start here so none equals the process id (the main-thread rule).
const TID_BASE: u64 = 1_000_000_000;

/// `TrackEvent.Type`.
pub const SLICE_BEGIN: u64 = 1;
pub const SLICE_END: u64 = 2;
pub const INSTANT: u64 = 3;
pub const COUNTER: u64 = 4;

/// `TracePacket.sequence_flags`.
const SEQ_INCREMENTAL_STATE_CLEARED: u64 = 1;
const SEQ_NEEDS_INCREMENTAL_STATE: u64 = 2;

// ---- protobuf writing -------------------------------------------------------------------

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn key(out: &mut Vec<u8>, field: u32, wire: u8) {
    varint(out, (u64::from(field) << 3) | u64::from(wire));
}

fn put_varint(out: &mut Vec<u8>, field: u32, v: u64) {
    key(out, field, 0);
    varint(out, v);
}

fn put_bytes(out: &mut Vec<u8>, field: u32, b: &[u8]) {
    key(out, field, 2);
    varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

fn put_double(out: &mut Vec<u8>, field: u32, v: f64) {
    key(out, field, 1);
    out.extend_from_slice(&v.to_bits().to_le_bytes());
}

fn packet(trace: &mut Vec<u8>, body: &[u8]) {
    put_bytes(trace, 1, body);
}

fn descriptor(trace: &mut Vec<u8>, desc: &[u8], first: bool) {
    let mut p = Vec::with_capacity(desc.len() + 8);
    put_varint(&mut p, 10, SEQ);
    if first {
        put_varint(&mut p, 13, SEQ_INCREMENTAL_STATE_CLEARED);
    }
    put_bytes(&mut p, 60, desc);
    packet(trace, &p);
}

struct Ev<'a> {
    ts: u64,
    kind: u64,
    track: u64,
    name: Option<&'a str>,
    value: Option<f64>,
}

fn event(trace: &mut Vec<u8>, scratch: &mut Vec<u8>, e: &Ev<'_>) {
    scratch.clear();
    put_varint(scratch, 9, e.kind);
    put_varint(scratch, 11, e.track);
    if let Some(n) = e.name {
        put_bytes(scratch, 23, n.as_bytes());
    }
    if let Some(v) = e.value {
        put_double(scratch, 44, v);
    }
    let mut p = Vec::with_capacity(scratch.len() + 16);
    put_varint(&mut p, 8, e.ts);
    put_varint(&mut p, 10, SEQ);
    put_varint(&mut p, 13, SEQ_NEEDS_INCREMENTAL_STATE);
    put_bytes(&mut p, 11, scratch);
    packet(trace, &p);
}

/// One thread's zones as a properly nested begin/end sequence in time order: slices sorted
/// by start (longer first at equal starts), a stack closing every slice that ended before the
/// next begins. A slice overlapping its parent's end (impossible for RAII zones on one
/// thread) is clipped to the parent so the output still nests.
fn nest(mut slices: Vec<&Slice>) -> Vec<(u64, bool, &'static str)> {
    slices.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut out = Vec::with_capacity(slices.len() * 2);
    let mut stack: Vec<u64> = Vec::new();
    for s in slices {
        while let Some(&top) = stack.last() {
            if top <= s.start {
                out.push((top, false, ""));
                stack.pop();
            } else {
                break;
            }
        }
        let end = stack.last().map_or(s.end, |&top| s.end.min(top));
        out.push((s.start, true, s.name));
        stack.push(end);
    }
    while let Some(top) = stack.pop() {
        out.push((top, false, ""));
    }
    out
}

pub(crate) fn encode(rec: &Recording, process_name: &str, pid: u32) -> Vec<u8> {
    let pid = u64::from(pid & 0x7fff_ffff);
    let mut trace = Vec::with_capacity(64 + rec.len() * 24);

    // Descriptors.
    let mut proc_ = Vec::new();
    put_varint(&mut proc_, 1, pid);
    put_bytes(&mut proc_, 6, process_name.as_bytes());
    let mut d = Vec::new();
    put_varint(&mut d, 1, PROCESS_UUID);
    put_bytes(&mut d, 3, &proc_);
    descriptor(&mut trace, &d, true);

    for (tid, name) in &rec.threads {
        let mut th = Vec::new();
        put_varint(&mut th, 1, pid);
        put_varint(&mut th, 2, TID_BASE + u64::from(*tid));
        put_bytes(&mut th, 5, name.as_bytes());
        let mut d = Vec::new();
        put_varint(&mut d, 1, THREAD_UUID + u64::from(*tid));
        put_varint(&mut d, 5, PROCESS_UUID);
        put_bytes(&mut d, 4, &th);
        descriptor(&mut trace, &d, false);
    }

    let mut counter_tracks: BTreeMap<&str, u64> = BTreeMap::new();
    for c in &rec.counters {
        let n = counter_tracks.len() as u64;
        counter_tracks.entry(c.name).or_insert(COUNTER_UUID + n);
    }
    for (name, uuid) in &counter_tracks {
        let mut d = Vec::new();
        put_varint(&mut d, 1, *uuid);
        put_bytes(&mut d, 2, name.as_bytes());
        put_varint(&mut d, 5, PROCESS_UUID);
        put_bytes(&mut d, 8, &[]);
        descriptor(&mut trace, &d, false);
    }

    // Events: per-thread nested slices, then counters and instants, merged by time with a
    // stable sort (a thread's own begin/end order survives equal timestamps).
    let mut by_thread: BTreeMap<u32, Vec<&Slice>> = BTreeMap::new();
    for s in &rec.slices {
        by_thread.entry(s.tid).or_default().push(s);
    }
    let mut evs: Vec<Ev<'_>> = Vec::with_capacity(rec.len() * 2);
    for (tid, slices) in by_thread {
        let track = THREAD_UUID + u64::from(tid);
        for (ts, begin, name) in nest(slices) {
            evs.push(Ev {
                ts,
                kind: if begin { SLICE_BEGIN } else { SLICE_END },
                track,
                name: begin.then_some(name),
                value: None,
            });
        }
    }
    for c in &rec.counters {
        evs.push(Ev {
            ts: c.ts,
            kind: COUNTER,
            track: counter_tracks.get(c.name).copied().unwrap_or(COUNTER_UUID),
            name: None,
            value: Some(c.value),
        });
    }
    for i in &rec.instants {
        evs.push(Ev {
            ts: i.ts,
            kind: INSTANT,
            track: THREAD_UUID + u64::from(i.tid),
            name: Some(&i.name),
            value: None,
        });
    }
    evs.sort_by_key(|e| e.ts);
    let mut scratch = Vec::new();
    for e in &evs {
        event(&mut trace, &mut scratch, e);
    }
    if rec.dropped > 0 {
        // Say so in the trace itself: a truncated recording must not look complete.
        let ts = evs.last().map_or(0, |e| e.ts);
        let name = format!("forge-trace: {} events dropped at the cap", rec.dropped);
        event(
            &mut trace,
            &mut scratch,
            &Ev {
                ts,
                kind: INSTANT,
                track: PROCESS_UUID,
                name: Some(&name),
                value: None,
            },
        );
    }
    trace
}

// ---- protobuf reading -------------------------------------------------------------------

/// A protobuf field value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

fn read_varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*i)?;
        *i += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// Decode one protobuf message into `(field, value)` pairs, in order. `None` if malformed.
#[must_use]
pub fn fields(b: &[u8]) -> Option<Vec<(u32, Field<'_>)>> {
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let k = read_varint(b, &mut i)?;
        let field = u32::try_from(k >> 3).ok()?;
        let v = match k & 7 {
            0 => Field::Varint(read_varint(b, &mut i)?),
            1 => {
                let s = b.get(i..i + 8)?;
                i += 8;
                Field::Fixed64(u64::from_le_bytes(s.try_into().ok()?))
            }
            2 => {
                let n = usize::try_from(read_varint(b, &mut i)?).ok()?;
                let s = b.get(i..i.checked_add(n)?)?;
                i += n;
                Field::Bytes(s)
            }
            5 => {
                let s = b.get(i..i + 4)?;
                i += 4;
                Field::Fixed32(u32::from_le_bytes(s.try_into().ok()?))
            }
            _ => return None,
        };
        out.push((field, v));
    }
    Some(out)
}

/// A decoded track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub uuid: u64,
    pub parent: Option<u64>,
    /// The track's name, or its thread's / process's name.
    pub name: String,
    pub kind: TrackKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Process,
    Thread,
    Counter,
    Other,
}

/// A decoded track event.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub ts: u64,
    /// [`SLICE_BEGIN`], [`SLICE_END`], [`INSTANT`] or [`COUNTER`].
    pub kind: u64,
    pub track: u64,
    pub name: Option<String>,
    pub value: Option<f64>,
    pub sequence: u64,
    pub flags: u64,
}

/// A decoded trace: its tracks and its events, in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decoded {
    pub tracks: Vec<Track>,
    pub events: Vec<Event>,
    /// Sequence flags of the first packet.
    pub first_flags: u64,
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Decode a trace written by [`crate::Tracer::take_perfetto`]. `None` if it is not a
/// well-formed protobuf of that shape.
#[must_use]
pub fn read(trace: &[u8]) -> Option<Decoded> {
    let mut d = Decoded::default();
    for (n, (f, v)) in fields(trace)?.into_iter().enumerate() {
        let (1, Field::Bytes(p)) = (f, v) else {
            return None;
        };
        let mut ts = 0;
        let mut seq = 0;
        let mut flags = 0;
        let mut te = None;
        let mut td = None;
        for (f, v) in fields(p)? {
            match (f, v) {
                (8, Field::Varint(x)) => ts = x,
                (10, Field::Varint(x)) => seq = x,
                (13, Field::Varint(x)) => flags = x,
                (11, Field::Bytes(b)) => te = Some(b),
                (60, Field::Bytes(b)) => td = Some(b),
                _ => {}
            }
        }
        if n == 0 {
            d.first_flags = flags;
        }
        if let Some(b) = td {
            let mut t = Track {
                uuid: 0,
                parent: None,
                name: String::new(),
                kind: TrackKind::Other,
            };
            for (f, v) in fields(b)? {
                match (f, v) {
                    (1, Field::Varint(x)) => t.uuid = x,
                    (2, Field::Bytes(s)) => t.name = text(s),
                    (5, Field::Varint(x)) => t.parent = Some(x),
                    (8, Field::Bytes(_)) => t.kind = TrackKind::Counter,
                    (3, Field::Bytes(s)) => {
                        t.kind = TrackKind::Process;
                        for (f, v) in fields(s)? {
                            if let (6, Field::Bytes(n)) = (f, v) {
                                t.name = text(n);
                            }
                        }
                    }
                    (4, Field::Bytes(s)) => {
                        t.kind = TrackKind::Thread;
                        for (f, v) in fields(s)? {
                            if let (5, Field::Bytes(n)) = (f, v) {
                                t.name = text(n);
                            }
                        }
                    }
                    _ => {}
                }
            }
            d.tracks.push(t);
        }
        if let Some(b) = te {
            let mut e = Event {
                ts,
                kind: 0,
                track: 0,
                name: None,
                value: None,
                sequence: seq,
                flags,
            };
            for (f, v) in fields(b)? {
                match (f, v) {
                    (9, Field::Varint(x)) => e.kind = x,
                    (11, Field::Varint(x)) => e.track = x,
                    (23, Field::Bytes(s)) => e.name = Some(text(s)),
                    (44, Field::Fixed64(x)) => e.value = Some(f64::from_bits(x)),
                    _ => {}
                }
            }
            d.events.push(e);
        }
    }
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(tid: u32, name: &'static str, start: u64, end: u64) -> Slice {
        Slice {
            tid,
            name,
            start,
            end,
        }
    }

    #[test]
    fn nesting_is_proper_whatever_the_close_order() {
        // Inner zones close (and are recorded) before their parents.
        let v = [
            s(1, "c", 2, 3),
            s(1, "b", 1, 4),
            s(1, "d", 4, 6),
            s(1, "a", 0, 10),
            s(1, "z", 10, 10),
        ];
        let seq = nest(v.iter().collect());
        let names: Vec<String> = seq
            .iter()
            .map(|(t, b, n)| format!("{}{}@{t}", if *b { "+" } else { "-" }, n))
            .collect();
        assert_eq!(
            names,
            [
                "+a@0", "+b@1", "+c@2", "-@3", "-@4", "+d@4", "-@6", "-@10", "+z@10", "-@10"
            ]
        );
    }

    #[test]
    fn varints_round_trip() {
        for v in [0u64, 1, 127, 128, 300, u64::from(u32::MAX), u64::MAX] {
            let mut b = Vec::new();
            varint(&mut b, v);
            let mut i = 0;
            assert_eq!(read_varint(&b, &mut i), Some(v));
            assert_eq!(i, b.len());
        }
        assert_eq!(fields(&[0x80]), None, "a truncated varint is refused");
    }
}
