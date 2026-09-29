//! Triggers (Ch.28 §28.12): when an actuated action *fires*. Every frame each trigger of an
//! action yields `None`, `Ongoing` or `Triggered`; the action's phase events (started,
//! triggered, completed, canceled) follow from that result against the previous frame's.
//!
//! | Trigger | Ongoing while | Triggered |
//! |---|---|---|
//! | `Down` (default) | — | every frame the action is actuated |
//! | `Pressed` | — | the frame it becomes actuated |
//! | `Released` | actuated | the frame it is released |
//! | `Hold(t)` / `Hold(t, repeat)` | held for less than `t` s | once at `t` (every frame after, with `repeat`) |
//! | `Tap(max)` | held for at most `max` s | on release within `max` |
//! | `MultiTap(n, gap)` | within a sequence | on the `n`-th **press** (each press within `gap` s of the previous release) |
//! | `Pulse(interval)` | held | on the press, then every `interval` s held |
//! | `Chord(map/action)` | — | *implicit*: only while the other action is triggered |
//! | `Combo(map/a w, map/b w, ...)` | part-way through | when each step's action fired in order, each within its window `w` s of the previous |
//!
//! Explicit triggers (every one but `Chord`) combine as *any*: one triggered is enough.
//! Implicit ones (`Chord`) combine as *all*: a missing chord holds the action at `Ongoing`.
//! A multi-tap fires on the press that completes it, not a release later (the player feels
//! the double-tap when they make it). A combo resets when a step's window runs out or when
//! another of its steps fires out of order.

use std::fmt;

use crate::faults::InputFaults;

/// An authored trigger. Action references are `map/action` ids.
#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    Down,
    Pressed,
    Released,
    Hold { time: f32, repeat: bool },
    Tap { max: f32 },
    MultiTap { count: u8, gap: f32 },
    Pulse { interval: f32 },
    Chord(String),
    Combo(Vec<(String, f32)>),
}

impl Trigger {
    /// Implicit (all must pass) rather than explicit (any fires).
    pub fn implicit(&self) -> bool {
        matches!(self, Trigger::Chord(_))
    }

    /// The actions this trigger reads.
    pub fn depends_on(&self) -> Vec<&str> {
        match self {
            Trigger::Chord(a) => vec![a.as_str()],
            Trigger::Combo(steps) => steps.iter().map(|(a, _)| a.as_str()).collect(),
            _ => Vec::new(),
        }
    }

    pub fn parse(s: &str) -> Result<Trigger, String> {
        let s = s.trim();
        let (head, args): (&str, Vec<&str>) = match s.split_once('(') {
            Some((h, rest)) => (
                h.trim(),
                rest.strip_suffix(')')
                    .ok_or_else(|| format!("{s:?}: missing ')'"))?
                    .split(',')
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .collect(),
            ),
            None => (s, Vec::new()),
        };
        let secs = |a: Option<&&str>, what: &str| -> Result<f32, String> {
            let a = a.ok_or_else(|| format!("{s:?}: needs {what}"))?;
            a.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite() && *v > 0.0)
                .ok_or_else(|| format!("{s:?}: {what} must be a positive number of seconds"))
        };
        let path = |a: &str| -> Result<String, String> {
            let ok = a.split_once('/').is_some_and(|(m, x)| {
                !m.is_empty()
                    && !x.is_empty()
                    && a.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '/')
            });
            if ok {
                Ok(a.to_string())
            } else {
                Err(format!("{s:?}: {a:?} is not a map/action id"))
            }
        };
        match head {
            "Down" => Ok(Trigger::Down),
            "Pressed" => Ok(Trigger::Pressed),
            "Released" => Ok(Trigger::Released),
            "Hold" => Ok(Trigger::Hold {
                time: secs(args.first(), "a hold time")?,
                repeat: match args.get(1).copied() {
                    None => false,
                    Some("repeat") => true,
                    Some(o) => return Err(format!("{s:?}: {o:?} (only `repeat`)")),
                },
            }),
            "Tap" => Ok(Trigger::Tap {
                max: secs(args.first(), "a maximum press time")?,
            }),
            "MultiTap" => {
                let count = args
                    .first()
                    .and_then(|a| a.parse::<u8>().ok())
                    .filter(|n| *n >= 2)
                    .ok_or_else(|| format!("{s:?}: a multi-tap needs a count of 2 or more"))?;
                Ok(Trigger::MultiTap {
                    count,
                    gap: secs(args.get(1), "a gap")?,
                })
            }
            "Pulse" => Ok(Trigger::Pulse {
                interval: secs(args.first(), "an interval")?,
            }),
            "Chord" => {
                let a = args
                    .first()
                    .ok_or_else(|| format!("{s:?}: a chord names an action"))?;
                Ok(Trigger::Chord(path(a)?))
            }
            "Combo" => {
                let mut steps = Vec::new();
                for a in &args {
                    let mut it = a.split_whitespace();
                    let (Some(p), w, None) = (it.next(), it.next(), it.next()) else {
                        return Err(format!("{s:?}: a combo step is `map/action window`"));
                    };
                    let w = match w {
                        Some(w) => secs(Some(&w), "a step window")?,
                        None => 0.5,
                    };
                    steps.push((path(p)?, w));
                }
                if steps.len() < 2 {
                    return Err(format!("{s:?}: a combo needs two or more steps"));
                }
                Ok(Trigger::Combo(steps))
            }
            o => Err(format!("unknown trigger {o:?}")),
        }
    }

    pub fn parse_chain(s: &str) -> Result<Vec<Trigger>, String> {
        s.split('|')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(Trigger::parse)
            .collect()
    }

    pub fn chain_text(t: &[Trigger]) -> String {
        t.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trigger::Down => write!(f, "Down"),
            Trigger::Pressed => write!(f, "Pressed"),
            Trigger::Released => write!(f, "Released"),
            Trigger::Hold {
                time,
                repeat: false,
            } => write!(f, "Hold({time})"),
            Trigger::Hold { time, repeat: true } => write!(f, "Hold({time}, repeat)"),
            Trigger::Tap { max } => write!(f, "Tap({max})"),
            Trigger::MultiTap { count, gap } => write!(f, "MultiTap({count}, {gap})"),
            Trigger::Pulse { interval } => write!(f, "Pulse({interval})"),
            Trigger::Chord(a) => write!(f, "Chord({a})"),
            Trigger::Combo(steps) => {
                write!(f, "Combo(")?;
                for (i, (a, w)) in steps.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a} {w}")?;
                }
                write!(f, ")")
            }
        }
    }
}

/// A trigger's result for one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum TResult {
    #[default]
    None,
    Ongoing,
    Triggered,
}

/// A compiled trigger: action references resolved to action indices.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CTrigger {
    Down,
    Pressed,
    Released,
    Hold { time: f32, repeat: bool },
    Tap { max: f32 },
    MultiTap { count: u8, gap: f32 },
    Pulse { interval: f32 },
    Chord(u32),
    Combo(Vec<(u32, f32)>),
}

impl CTrigger {
    pub(crate) fn implicit(&self) -> bool {
        matches!(self, CTrigger::Chord(_))
    }
}

/// A trigger's per-player state (plain numbers: no allocation per frame).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TState {
    /// Seconds held (hold, tap, pulse), or since the last step (combo) / release (multi-tap).
    pub t: f32,
    /// The next pulse time.
    pub next: f32,
    /// Taps counted, or the combo's next step.
    pub n: u8,
    /// A one-shot hold already fired.
    pub fired: bool,
}

/// What a trigger sees this frame.
pub(crate) struct TIn {
    pub actuated: bool,
    pub was: bool,
    pub dt: f32,
}

/// What other actions did this frame (chords and combos read them).
pub(crate) trait Others {
    /// Its result this frame (last frame's, if it is evaluated later).
    fn result(&self, action: u32) -> TResult;
    /// It went to Triggered this frame from not triggered.
    fn fresh(&self, action: u32) -> bool;
}

pub(crate) fn step(
    t: &CTrigger,
    s: &mut TState,
    i: &TIn,
    others: &dyn Others,
    f: &InputFaults,
) -> TResult {
    use TResult::{None as N, Ongoing as O, Triggered as T};
    let press = i.actuated && !i.was;
    let release = !i.actuated && i.was;
    match t {
        CTrigger::Down => {
            if i.actuated {
                T
            } else {
                N
            }
        }
        CTrigger::Pressed => {
            if press {
                T
            } else {
                N
            }
        }
        CTrigger::Released => {
            if i.actuated {
                O
            } else if release {
                T
            } else {
                N
            }
        }
        CTrigger::Hold { time, repeat } => {
            if !i.actuated {
                s.t = 0.0;
                s.fired = false;
                return N;
            }
            if press {
                s.t = 0.0;
                s.fired = false;
            } else {
                s.t += i.dt;
            }
            if s.t >= *time || f.hold_ignores_time() {
                if *repeat {
                    T
                } else if !s.fired {
                    s.fired = true;
                    T
                } else {
                    N
                }
            } else {
                O
            }
        }
        CTrigger::Tap { max } => {
            if i.actuated {
                if press {
                    s.t = 0.0;
                } else {
                    s.t += i.dt;
                }
                if s.t <= *max || f.tap_ignores_max() {
                    O
                } else {
                    N
                }
            } else if release && (s.t <= *max || f.tap_ignores_max()) {
                T
            } else {
                N
            }
        }
        CTrigger::MultiTap { count, gap } => {
            let late = |t: f32| t > *gap && !f.multitap_ignores_gap();
            if press {
                if s.n > 0 && late(s.t) {
                    s.n = 0;
                }
                s.n = s.n.saturating_add(1);
                s.t = 0.0;
                if s.n >= *count {
                    s.n = 0;
                    T
                } else {
                    O
                }
            } else if s.n == 0 {
                N
            } else if release {
                s.t = 0.0;
                O
            } else {
                s.t += i.dt;
                if late(s.t) {
                    s.n = 0;
                    N
                } else {
                    O
                }
            }
        }
        CTrigger::Pulse { interval } => {
            if !i.actuated {
                return N;
            }
            if press {
                s.t = 0.0;
                s.next = *interval;
                return T;
            }
            s.t += i.dt;
            if s.t >= s.next {
                s.next += *interval;
                T
            } else {
                O
            }
        }
        CTrigger::Chord(a) => {
            if f.chord_ignored() || others.result(*a) == T {
                T
            } else {
                N
            }
        }
        CTrigger::Combo(steps) => {
            s.t += i.dt;
            let at = usize::from(s.n);
            if at > 0 && steps.get(at).is_some_and(|(_, w)| s.t > *w) {
                s.n = 0;
            }
            let at = usize::from(s.n);
            let Some((want, _)) = steps.get(at) else {
                s.n = 0;
                return N;
            };
            let advance = if f.combo_ignores_order() {
                steps.iter().any(|(a, _)| others.fresh(*a))
            } else {
                others.fresh(*want)
            };
            if advance {
                s.n += 1;
                s.t = 0.0;
                if usize::from(s.n) >= steps.len() {
                    s.n = 0;
                    return T;
                }
                return O;
            }
            if s.n > 0 && steps.iter().any(|(a, _)| a != want && others.fresh(*a)) {
                // Out of order: start again (the stray press may itself be step one).
                s.n = u8::from(steps.first().is_some_and(|(a, _)| others.fresh(*a)));
                s.t = 0.0;
            }
            if s.n > 0 { O } else { N }
        }
    }
}

/// Combine a frame's trigger results (explicit: any; implicit: all).
pub(crate) fn combine(explicit: TResult, any_explicit: bool, implicit_ok: bool) -> TResult {
    let e = if any_explicit {
        explicit
    } else {
        TResult::None
    };
    if implicit_ok {
        e
    } else if e == TResult::None {
        TResult::None
    } else {
        TResult::Ongoing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chains_round_trip() {
        let s = "Hold(0.5) | Hold(0.3, repeat) | Tap(0.2) | MultiTap(2, 0.3) | Pulse(0.25) | Chord(gameplay/aim) | Combo(gameplay/light 0.5, gameplay/heavy 0.4) | Pressed | Released | Down";
        let t = Trigger::parse_chain(s).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(t.len(), 10);
        assert_eq!(Trigger::chain_text(&t), s);
        for bad in [
            "Hold",
            "Hold(-1)",
            "MultiTap(1, 0.3)",
            "Chord(aim)",
            "Combo(a/b 0.3)",
            "Jump",
        ] {
            assert!(Trigger::parse(bad).is_err(), "{bad}");
        }
    }
}
