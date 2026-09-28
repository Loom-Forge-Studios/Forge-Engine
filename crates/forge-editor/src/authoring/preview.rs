//! The timeline preview: scrubbing and playing a clip **through the real play core**
//! (WP-13's `forge-sim`), as session state — the project is never written by a preview.
//!
//! * **Scrub** evaluates the clip at the playhead, writes the sampled values over the
//!   animated entities' rows of the edit world, and forks a [`SimWorld`] from exactly those
//!   rows: the play core turns the keyed transform properties (`transform.position.local`,
//!   the frame, yaw/pitch/roll, scale) into the transforms the viewport draws, the same
//!   way Play does. Only the animated entities are forked, so a scrub costs the clip, not
//!   the scene.
//! * **Play** advances the playhead on the play core's own fixed 60 Hz step schedule
//!   (`forge_sim::tick_of_step`): the times sampled are the times a simulation step would
//!   sample, whatever the editor's frame rate, so a preview is reproducible. Event keys
//!   the playhead crosses are collected ([`TimelinePreview::take_events`]).
//! * **Stop** drops the overrides; the viewport shows the edit world again.
//!
//! The viewport follows [`TimelinePreview::overrides`] while the play core is stopped (a
//! running play session wins: it is the sandbox the user asked to see).

use std::collections::BTreeMap;
use std::time::Duration;

use forge_cmd::{EntityKey, Value};
use forge_sim::edit::{EditEntity, EditSnapshot};
use forge_sim::{SIM_RATE_HZ, SimTransform, SimWorld, tick_of_step};

use super::timeline::Clip;
use crate::mirror::ProjectMirror;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Clock {
    started_at: Duration,
    from: f64,
    steps: u64,
}

/// See the module docs.
#[derive(Debug, Default)]
pub struct TimelinePreview {
    clip: Option<String>,
    time: f64,
    clock: Option<Clock>,
    transforms: BTreeMap<EntityKey, SimTransform>,
    values: Vec<(EntityKey, String, Value)>,
    events: Vec<(f64, String, String)>,
    revision: u64,
    forks: u64,
    error: Option<String>,
}

impl TimelinePreview {
    pub fn new() -> Self {
        Self::default()
    }
    /// The clip being previewed, if any.
    pub fn clip(&self) -> Option<&str> {
        self.clip.as_deref()
    }
    /// The playhead (seconds, clip time).
    pub fn time(&self) -> f64 {
        self.time
    }
    pub fn is_playing(&self) -> bool {
        self.clock.is_some()
    }
    /// Bumped whenever what the preview shows changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// How many times the play core was forked (a scrub is one fork; the guard reads it).
    pub fn forks(&self) -> u64 {
        self.forks
    }
    /// The last evaluation the play core refused (a malformed transform), taken once.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
    /// The sampled values at the playhead: `(entity, path, value)`.
    pub fn values(&self) -> &[(EntityKey, String, Value)] {
        &self.values
    }
    /// The transforms the viewport draws while previewing, with the revision.
    pub fn overrides(&self) -> Option<(&BTreeMap<EntityKey, SimTransform>, u64)> {
        self.clip
            .as_ref()
            .map(|_| (&self.transforms, self.revision))
    }
    /// Event keys the playhead crossed while playing, since the last call.
    pub fn take_events(&mut self) -> Vec<(f64, String, String)> {
        std::mem::take(&mut self.events)
    }

    /// Show `clip` at `t` (pauses a playing preview).
    pub fn scrub(&mut self, m: &ProjectMirror, clip: &Clip, t: f64) {
        self.clock = None;
        self.clip = Some(clip.id.clone());
        self.time = clip.local_time(t);
        self.evaluate(m, clip);
    }

    /// Re-evaluate at the current playhead (the clip or the project changed).
    pub fn refresh(&mut self, m: &ProjectMirror, clip: &Clip) {
        if self.clip.as_deref() == Some(clip.id.as_str()) {
            self.time = clip.local_time(self.time);
            self.evaluate(m, clip);
        }
    }

    fn evaluate(&mut self, m: &ProjectMirror, clip: &Clip) {
        self.values = clip.sample(self.time);
        let mut rows: BTreeMap<EntityKey, EditEntity> = BTreeMap::new();
        for k in clip.animated() {
            if let Some(e) = m.entity(k) {
                rows.insert(
                    k,
                    EditEntity {
                        key: k,
                        name: e.name.clone(),
                        parent: e.parent,
                        properties: e.properties.clone(),
                    },
                );
            }
        }
        for (k, path, v) in &self.values {
            if let Some(r) = rows.get_mut(k) {
                r.properties.insert(path.clone(), v.clone());
            }
        }
        let snap = EditSnapshot::from_entities(rows.into_values());
        self.forks += 1;
        match SimWorld::fork(&snap) {
            Ok(w) => {
                self.transforms = snap
                    .entities
                    .iter()
                    .filter_map(|e| w.transform_of(e.key).map(|t| (e.key, t)))
                    .collect();
            }
            Err(e) => {
                self.transforms.clear();
                self.error = Some(e.to_string());
            }
        }
        self.revision += 1;
    }

    /// Start playing from the playhead at `now` (the editor's clock).
    pub fn play(&mut self, m: &ProjectMirror, clip: &Clip, now: Duration) {
        if self.clip.as_deref() != Some(clip.id.as_str()) {
            self.scrub(m, clip, 0.0);
        }
        if !clip.looping && self.time >= clip.length {
            self.time = 0.0;
        }
        self.clock = Some(Clock {
            started_at: now,
            from: self.time,
            steps: 0,
        });
        self.revision += 1;
    }

    pub fn pause(&mut self) {
        if self.clock.take().is_some() {
            self.revision += 1;
        }
    }

    /// Stop previewing: the viewport shows the edit world again.
    pub fn stop(&mut self) {
        if self.clip.take().is_some() || self.clock.is_some() {
            self.clock = None;
            self.transforms.clear();
            self.values.clear();
            self.revision += 1;
        }
    }

    /// Advance a playing preview to `now` in whole play-core steps. Returns whether the
    /// playhead moved. A clip that does not loop pauses at its end.
    pub fn advance(&mut self, m: &ProjectMirror, clip: &Clip, now: Duration) -> bool {
        let Some(mut c) = self.clock else {
            return false;
        };
        if self.clip.as_deref() != Some(clip.id.as_str()) {
            return false;
        }
        let elapsed = now.saturating_sub(c.started_at).as_micros();
        let steps =
            u64::try_from(elapsed * u128::from(SIM_RATE_HZ) / 1_000_000).unwrap_or(u64::MAX);
        if steps <= c.steps {
            return false;
        }
        let at = |n: u64| c.from + tick_of_step(n).0 as f64 / 1e6;
        let prev = at(c.steps);
        c.steps = steps;
        let t = at(steps);
        let (t, ended) = if !clip.looping && t >= clip.length {
            (clip.length, true)
        } else {
            (t, false)
        };
        // Every event between the last step shown and this one, even over a stalled frame.
        self.events.extend(clip.events_between(prev.min(t), t));
        self.time = clip.local_time(t);
        self.clock = (!ended).then_some(c);
        self.evaluate(m, clip);
        true
    }
}
