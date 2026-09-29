//! The input debugger's view of a runtime (Ch.28 §28.18): devices and their live controls,
//! players with their devices, contexts and every action's value and phase, and the recent
//! raw events. [`InputRuntime::debug_snapshot`] builds it on request (the editor's Input
//! debugger panel asks at most 20 times a second while it is visible); the runtime records
//! events only while the log is on, so a game that never opens the debugger pays nothing.

use crate::control::{Device, control_count, control_name};
use crate::glyph::PadFamily;
use crate::map::ActionHandle;
use crate::runtime::{ActionState, DeviceId, InputRuntime, InputStats, Phase, PlayerId};

/// A device and its non-zero controls.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceView {
    pub id: DeviceId,
    pub class: Device,
    pub name: String,
    pub family: PadFamily,
    pub connected: bool,
    pub is_virtual: bool,
    pub player: Option<PlayerId>,
    /// `(control, value)` for every control not at rest.
    pub active: Vec<(String, [f32; 2])>,
    /// Frames since its last event (`None`: never).
    pub idle_frames: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionView {
    pub action: ActionHandle,
    pub path: String,
    pub name: String,
    pub state: ActionState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextView {
    pub id: String,
    pub name: String,
    pub priority: i32,
    pub enabled: bool,
    pub actions: Vec<ActionView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerView {
    pub id: PlayerId,
    pub devices: Vec<DeviceId>,
    pub contexts: Vec<ContextView>,
    pub overrides: usize,
}

/// One logged event, readable.
#[derive(Clone, Debug, PartialEq)]
pub struct EventView {
    pub frame: u64,
    pub device: DeviceId,
    pub control: String,
    pub value: [f32; 2],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DebugSnapshot {
    pub frame: u64,
    pub devices: Vec<DeviceView>,
    pub players: Vec<PlayerView>,
    pub events: Vec<EventView>,
    pub stats: InputStats,
    pub problems: Vec<String>,
    /// Whether an action map is loaded.
    pub has_map: bool,
}

impl DebugSnapshot {
    /// The actions that are not idle, as `(player, path, phase, value)` (the panel's
    /// "active" filter).
    pub fn active_actions(&self) -> Vec<(PlayerId, &str, Phase, [f32; 2])> {
        let mut out = Vec::new();
        for p in &self.players {
            for c in &p.contexts {
                for a in &c.actions {
                    if a.state.phase != Phase::Idle || a.state.value != [0.0; 2] {
                        out.push((p.id, a.path.as_str(), a.state.phase, a.state.value));
                    }
                }
            }
        }
        out
    }
}

impl InputRuntime {
    /// Build the debugger's snapshot (allocates; call it from a tool, not every frame).
    pub fn debug_snapshot(&self) -> DebugSnapshot {
        let mut s = DebugSnapshot {
            frame: self.frame,
            stats: self.stats,
            problems: self.map.problems.clone(),
            has_map: self.has_map,
            ..DebugSnapshot::default()
        };
        for (i, d) in self.devices.iter().enumerate() {
            let class = d.desc.class;
            let mut active = Vec::new();
            for c in 0..control_count(class) {
                let Ok(c) = u16::try_from(c) else { continue };
                let v = d.values[usize::from(c)];
                if v != [0.0; 2] {
                    active.push((control_name(class, c).to_string(), v));
                }
            }
            s.devices.push(DeviceView {
                id: DeviceId(u32::try_from(i).unwrap_or(u32::MAX)),
                class,
                name: d.desc.name.clone(),
                family: d.desc.family,
                connected: d.connected,
                is_virtual: d.desc.is_virtual,
                player: d.player.map(PlayerId),
                active,
                idle_frames: (d.last_active > 0).then(|| self.frame - d.last_active),
            });
        }
        for (pi, p) in self.players.iter().enumerate() {
            if !p.active {
                continue;
            }
            let mut contexts = Vec::new();
            for (ci, c) in self.map.contexts.iter().enumerate() {
                let actions = self
                    .map
                    .actions
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| usize::from(a.ctx) == ci)
                    .map(|(ai, a)| ActionView {
                        action: ActionHandle(u32::try_from(ai).unwrap_or(u32::MAX)),
                        path: a.path.clone(),
                        name: a.name.clone(),
                        state: p.states.get(ai).copied().unwrap_or_default(),
                    })
                    .collect();
                contexts.push(ContextView {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    priority: c.priority,
                    enabled: p.enabled.get(ci).copied().unwrap_or(false),
                    actions,
                });
            }
            s.players.push(PlayerView {
                id: PlayerId(u8::try_from(pi).unwrap_or(u8::MAX)),
                devices: p.devices.clone(),
                contexts,
                overrides: p.overrides.map.values().map(|m| m.len()).sum(),
            });
        }
        if let Some((_, log)) = &self.log {
            for e in log.iter().rev() {
                let class = self
                    .devices
                    .get(e.device.0 as usize)
                    .map_or(Device::Keyboard, |d| d.desc.class);
                s.events.push(EventView {
                    frame: e.frame,
                    device: e.device,
                    control: format!("{}/{}", class.name(), control_name(class, e.control)),
                    value: e.value,
                });
            }
        }
        s
    }
}
