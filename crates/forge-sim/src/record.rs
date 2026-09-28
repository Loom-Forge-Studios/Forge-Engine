//! The replay file (`.forgereplay`) and [`replay`].
//!
//! **Format: JSON Lines, version 1.** Line 1 is the [`RecHeader`] — the format version, the
//! step rate, the checkpoint cadence, the edit world's hash and (by default) the edit world
//! itself, so a replay file alone reproduces its session. Every further line is one
//! [`RecEvent`], in the order it happened, stamped with the **simulation step** it took
//! effect at (never wall time): a play control, an input, a checkpoint hash every
//! `check_every` steps, and the final hash at Stop. Floats are written in shortest
//! round-trip form and read back exactly (`serde_json` with `float_roundtrip`, D-0001), so a
//! file re-serialises to the same bytes.
//!
//! Lines, not one document: a session is appended as it runs, a crash leaves a readable
//! prefix (replaying it verifies every checkpoint it reached), and two recordings diff line
//! by line.
//!
//! **Replay** re-runs the session from the edit world: controls and inputs are re-issued at
//! their recorded steps (steps in between are run directly — the recording says how many
//! there were, whatever the original frame rate was), the replaying session records itself,
//! and its recording must equal the original **event for event and hash for hash**; the first
//! difference is reported as `SIM-0007` with its step.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::SimError;
use crate::edit::EditSnapshot;
use crate::session::{PlayCommand, PlaySession};
use crate::world::{SIM_RATE_HZ, SimInput};

/// The replay format version this build writes and reads.
pub const FORMAT_VERSION: u32 = 1;

/// Line 1 of a replay file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecHeader {
    /// The format version ([`FORMAT_VERSION`]).
    pub forge_replay: u32,
    /// Simulation steps per second.
    pub rate_hz: u32,
    /// A checkpoint hash is recorded every this many steps.
    pub check_every: u64,
    /// [`EditSnapshot::hash`] of the edit world the session forked from.
    pub edit_hash: String,
    /// The edit world itself (absent when the recording was made without it; a replay then
    /// needs the scene from elsewhere, and checks it against `edit_hash`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<EditSnapshot>,
}

/// One recorded event, stamped with the step it took effect at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecEvent {
    /// A play control.
    Control { at: u64, cmd: PlayCommand },
    /// An input, recorded when issued; it applies at the start of the step after `at`.
    Input { at: u64, input: SimInput },
    /// The state hash after step `at`.
    Check { at: u64, hash: String },
    /// The final state hash at Stop.
    End { at: u64, hash: String },
}

impl RecEvent {
    /// The step it is stamped with.
    #[must_use]
    pub fn at(&self) -> u64 {
        match self {
            Self::Control { at, .. }
            | Self::Input { at, .. }
            | Self::Check { at, .. }
            | Self::End { at, .. } => *at,
        }
    }

    fn line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| format!("<unprintable: {e}>"))
    }
}

/// A recorded play session.
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    pub header: RecHeader,
    pub events: Vec<RecEvent>,
}

impl Recording {
    pub(crate) fn new(edit: &EditSnapshot, check_every: u64, embed: bool) -> Self {
        Self {
            header: RecHeader {
                forge_replay: FORMAT_VERSION,
                rate_hz: SIM_RATE_HZ,
                check_every,
                edit_hash: edit.hash(),
                edit: embed.then(|| edit.clone()),
            },
            events: Vec::new(),
        }
    }

    /// The step of the last event (the session's length).
    #[must_use]
    pub fn steps(&self) -> u64 {
        self.events.last().map_or(0, RecEvent::at)
    }

    /// The last recorded hash (the End hash of a stopped session, else the last checkpoint).
    #[must_use]
    pub fn final_hash(&self) -> Option<&str> {
        self.events.iter().rev().find_map(|e| match e {
            RecEvent::Check { hash, .. } | RecEvent::End { hash, .. } => Some(hash.as_str()),
            _ => None,
        })
    }

    /// Whether the session ended with Stop (a crash leaves no End line).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        matches!(self.events.last(), Some(RecEvent::End { .. }))
    }

    /// The file's text.
    #[must_use]
    pub fn to_jsonl(&self) -> String {
        let mut s = serde_json::to_string(&self.header).unwrap_or_default();
        s.push('\n');
        for e in &self.events {
            s.push_str(&e.line());
            s.push('\n');
        }
        s
    }

    /// Parse a replay file's text.
    pub fn from_jsonl(text: &str) -> Result<Recording, SimError> {
        let bad = |line: usize, detail: String| SimError::BadRecording { line, detail };
        let mut lines = text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty());
        let Some((n, first)) = lines.next() else {
            return Err(bad(1, "empty file".into()));
        };
        let header: RecHeader =
            serde_json::from_str(first).map_err(|e| bad(n + 1, format!("header: {e}")))?;
        if header.forge_replay != FORMAT_VERSION {
            return Err(bad(
                n + 1,
                format!(
                    "format version {} (this build reads {FORMAT_VERSION})",
                    header.forge_replay
                ),
            ));
        }
        if header.rate_hz != SIM_RATE_HZ {
            return Err(bad(
                n + 1,
                format!(
                    "recorded at {} Hz (this build steps at {SIM_RATE_HZ} Hz)",
                    header.rate_hz
                ),
            ));
        }
        if header.check_every == 0 {
            return Err(bad(n + 1, "check_every is 0".into()));
        }
        let mut events = Vec::new();
        let mut last = 0;
        for (n, l) in lines {
            let e: RecEvent = serde_json::from_str(l).map_err(|e| bad(n + 1, e.to_string()))?;
            if e.at() < last {
                return Err(bad(n + 1, format!("step {} after step {last}", e.at())));
            }
            last = e.at();
            events.push(e);
        }
        Ok(Recording { header, events })
    }

    /// Write the file.
    pub fn write(&self, path: &Path) -> Result<(), SimError> {
        std::fs::write(path, self.to_jsonl()).map_err(|e| SimError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })
    }

    /// Read a file.
    pub fn read(path: &Path) -> Result<Recording, SimError> {
        let text = std::fs::read_to_string(path).map_err(|e| SimError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        Self::from_jsonl(&text)
    }
}

/// What a successful replay did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    /// Steps simulated.
    pub steps: u64,
    /// Events re-issued and matched.
    pub events: usize,
    /// Hashes (checkpoints and the end) that matched.
    pub checks: usize,
    /// The replay's final state hash.
    pub final_hash: String,
}

/// Replay `rec` (see the module docs) against `scene` — or, when `None`, the edit world the
/// recording embeds. A given scene must hash to the recorded `edit_hash` (`SIM-0006`). The
/// replaying session reports to `session`'s tracer and uses its faults (none, normally).
pub fn replay(
    rec: &Recording,
    scene: Option<&EditSnapshot>,
    mut session: PlaySession,
) -> Result<ReplayReport, SimError> {
    let scene = match (scene, &rec.header.edit) {
        (Some(s), _) => {
            let actual = s.hash();
            if actual != rec.header.edit_hash {
                return Err(SimError::SceneMismatch {
                    recorded: rec.header.edit_hash.clone(),
                    actual,
                });
            }
            s
        }
        (None, Some(e)) => e,
        (None, None) => {
            return Err(SimError::BadRecording {
                line: 1,
                detail: "the file embeds no edit world; give the scene to replay against".into(),
            });
        }
    };
    session.set_check_every(rec.header.check_every);
    session.set_embed_edit(rec.header.edit.is_some());
    let diverged = |at: u64, expected: &RecEvent, got: Option<&RecEvent>| SimError::Diverged {
        step: at,
        expected: expected.line(),
        got: got.map_or_else(|| "nothing".to_owned(), RecEvent::line),
    };
    for ev in &rec.events {
        let at = ev.at();
        while session.step() < at && session.is_running() {
            session.run_steps(1)?;
        }
        let issued = matches!(ev, RecEvent::Control { .. } | RecEvent::Input { .. });
        if issued && session.step() != at {
            let got = session
                .recording()
                .and_then(|r| r.events.iter().find(|e| e.at() > at));
            return Err(diverged(at, ev, got));
        }
        match ev {
            RecEvent::Control { cmd, .. } => session.control(*cmd, scene)?,
            RecEvent::Input { input, .. } => session.input(*input)?,
            RecEvent::Check { .. } | RecEvent::End { .. } => {}
        }
    }
    let replayed = session.recording().cloned().unwrap_or_else(|| Recording {
        header: rec.header.clone(),
        events: Vec::new(),
    });
    for (i, ev) in rec.events.iter().enumerate() {
        let got = replayed.events.get(i);
        if got != Some(ev) {
            return Err(diverged(ev.at(), ev, got));
        }
    }
    if let Some(extra) = replayed.events.get(rec.events.len()) {
        return Err(SimError::Diverged {
            step: extra.at(),
            expected: "nothing".into(),
            got: extra.line(),
        });
    }
    let checks = rec
        .events
        .iter()
        .filter(|e| matches!(e, RecEvent::Check { .. } | RecEvent::End { .. }))
        .count();
    let final_hash = replayed
        .final_hash()
        .map(str::to_owned)
        .or_else(|| session.state_hash())
        .unwrap_or_default();
    Ok(ReplayReport {
        steps: rec.steps(),
        events: rec.events.len(),
        checks,
        final_hash,
    })
}
