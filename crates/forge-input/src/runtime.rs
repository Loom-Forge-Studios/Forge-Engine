//! The input runtime (Ch.28 §28.9): devices, players, the compiled map, and one `update` per
//! frame that turns raw device events into action states.
//!
//! **One frame.** Backends (winit, gilrs, touch, a test's virtual devices) push raw events
//! at any time; [`InputRuntime::update`] applies every event pushed since the last update,
//! then evaluates each player's enabled contexts. An event pushed before an update is
//! visible in *that* update's action states: zero frames of added latency (the
//! `input.latency.frames` budget, `test_input_budgets`). A press and its release inside one
//! frame still register as one frame of press (per-control edge bits), so a fast tap is never
//! lost.
//!
//! **Cost.** Nothing allocates in a steady frame (every buffer is reused; the allocation
//! guard counts), a control is an index into a table (no string compare), and a runtime with
//! no map or no player returns after draining its queue: an unused input runtime costs a
//! branch (`input.update.idle`).

use std::collections::BTreeMap;

use crate::control::{
    ActionKind, Behaviour, Binding, ControlRef, Device, Shape, control_behaviour, control_count,
    control_index, control_name, control_shape,
};
use crate::faults::InputFaults;
use crate::glyph::PadFamily;
use crate::haptics::{HapticsState, MotorCommand, Rumble};
use crate::map::{
    ActionHandle, BindingDef, CBinding, CContext, CompiledMap, ContextHandle, InputMapDef, Source,
};
use crate::modifier::Modifier;
use crate::motion::{GyroProcessor, GyroSettings, MotionSample};
use crate::touch::{GestureConfig, Gestures, TouchInput};
use crate::trigger::{self, Others, TIn, TResult, TState};

/// A device instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub u32);

/// A local player (0-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlayerId(pub u8);

/// The most local players.
pub const MAX_PLAYERS: usize = 8;

/// A button (or an axis read as a button) is pressed at this magnitude.
pub const PRESS_POINT: f32 = 0.5;
/// An axis action is actuated past this magnitude (anything its deadzone lets through).
pub const AXIS_ACTUATION: f32 = 1e-4;

/// What a backend says about a device when it connects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceDesc {
    pub class: Device,
    /// Shown in the debugger and the pairing prompt.
    pub name: String,
    pub family: PadFamily,
    /// Survives a reconnect (a pad's GUID, "keyboard"): a device that comes back with the same
    /// key goes back to its player.
    pub stable_key: String,
    /// A virtual device (a test's, or the on-screen controls').
    pub is_virtual: bool,
}

impl DeviceDesc {
    pub fn new(class: Device, name: &str, stable_key: &str) -> Self {
        Self {
            class,
            name: name.to_string(),
            family: PadFamily::Generic,
            stable_key: stable_key.to_string(),
            is_virtual: false,
        }
    }
    pub fn family(mut self, f: PadFamily) -> Self {
        self.family = f;
        self
    }
    pub fn virtual_device(mut self) -> Self {
        self.is_virtual = true;
        self
    }
}

/// One raw control change. A [`Behaviour::Delta`] control adds its value within a frame;
/// every other control is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawEvent {
    pub device: DeviceId,
    pub control: u16,
    pub value: [f32; 2],
}

pub(crate) struct DeviceSlot {
    pub desc: DeviceDesc,
    pub connected: bool,
    pub values: Vec<[f32; 2]>,
    /// Pressed / released this frame (bit per control).
    pub pressed: Vec<u64>,
    pub released: Vec<u64>,
    /// The class's delta controls (reset every frame).
    pub deltas: Vec<u16>,
    pub player: Option<u8>,
    pub gestures: Option<Gestures>,
    pub gyro: Option<GyroProcessor>,
    /// Frame of the last event (the debugger's activity column).
    pub last_active: u64,
}

impl DeviceSlot {
    fn new(desc: DeviceDesc) -> Self {
        let n = control_count(desc.class);
        let words = n.div_ceil(64);
        let class = desc.class;
        let deltas = (0..n)
            .filter_map(|i| u16::try_from(i).ok())
            .filter(|&i| control_behaviour(class, i) == Behaviour::Delta)
            .collect();
        Self {
            gestures: (class == Device::Touch).then(Gestures::default),
            desc,
            connected: true,
            values: vec![[0.0; 2]; n],
            pressed: vec![0; words],
            released: vec![0; words],
            deltas,
            player: None,
            gyro: None,
            last_active: 0,
        }
    }
    fn bit(v: &[u64], i: u16) -> bool {
        v.get(usize::from(i) / 64)
            .is_some_and(|w| w & (1u64 << (i % 64)) != 0)
    }
    fn set_bit(v: &mut [u64], i: u16) {
        if let Some(w) = v.get_mut(usize::from(i) / 64) {
            *w |= 1u64 << (i % 64);
        }
    }
}

fn mag(v: [f32; 2]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

/// An action's phase this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Ongoing,
    Triggered,
}

/// An action's state for one player, this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActionState {
    /// The value after modifiers (`[v, 0]` for a button or a 1D axis).
    pub value: [f32; 2],
    pub phase: Phase,
    /// Events this frame.
    pub started: bool,
    pub triggered: bool,
    pub completed: bool,
    pub canceled: bool,
    /// Seconds since it started (0 while idle).
    pub elapsed: f32,
    /// Times it has triggered (fresh triggers only: a held `Down` counts once).
    pub trigger_count: u32,
}

impl ActionState {
    /// The value as a scalar (x, or the magnitude of a 2D value).
    pub fn scalar(&self) -> f32 {
        if self.value[1] == 0.0 {
            self.value[0]
        } else {
            mag(self.value)
        }
    }
}

/// What happened to an action this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionEventKind {
    Started,
    Triggered,
    Completed,
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionEvent {
    pub player: PlayerId,
    pub action: ActionHandle,
    pub kind: ActionEventKind,
}

/// How devices find players (§28.14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinPolicy {
    /// One player owns every device (the single-player default).
    Single,
    /// A device with no player joins the next free player when one of its buttons is
    /// pressed (`button: None`) or when that control is (`Some(ControlRef)`, e.g.
    /// `Gamepad/Start`). A keyboard and the mice join together.
    AutoJoin {
        max_players: u8,
        button: Option<ControlRef>,
    },
    /// Only [`InputRuntime::assign`] pairs devices.
    Manual,
}

/// What happened to players this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerEvent {
    Joined { player: PlayerId, device: DeviceId },
    DeviceLost { player: PlayerId, device: DeviceId },
    DeviceRegained { player: PlayerId, device: DeviceId },
    Left { player: PlayerId },
}

/// Per-player binding overrides (§28.13): action path → slot → binding (`None`: unbound).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BindingOverrides {
    pub map: BTreeMap<String, BTreeMap<u32, Option<Binding>>>,
}

impl BindingOverrides {
    pub fn set(&mut self, action: &str, slot: u32, b: Option<Binding>) {
        self.map
            .entry(action.to_string())
            .or_default()
            .insert(slot, b);
    }
    pub fn get(&self, action: &str, slot: u32) -> Option<&Option<Binding>> {
        self.map.get(action).and_then(|m| m.get(&slot))
    }
    pub fn clear_action(&mut self, action: &str) {
        self.map.remove(action);
    }
    pub fn is_empty(&self) -> bool {
        self.map.values().all(BTreeMap::is_empty)
    }
}

/// How a rebinding treats a control another action of the context already uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Refuse, naming the other action.
    Refuse,
    /// Give the other action this slot's old binding.
    Swap,
    /// Bind anyway.
    Allow,
}

/// Which devices a rebinding listens to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceFilter {
    /// The player's own devices.
    Player,
    /// These classes of the player's devices (a settings screen's gamepad column, or its
    /// keyboard-and-mouse column).
    Classes(Vec<Device>),
}

/// An interactive rebinding (§28.13).
#[derive(Clone, Debug, PartialEq)]
pub struct RebindRequest {
    pub player: PlayerId,
    pub action: ActionHandle,
    pub slot: u32,
    /// The part of a composite to rebind (0 up/negative, 1 down/positive, 2 left, 3 right).
    pub part: Option<u8>,
    pub devices: DeviceFilter,
    /// Controls that cancel instead of binding.
    pub cancel: Vec<ControlRef>,
    pub conflicts: ConflictPolicy,
    /// Seconds to wait; `None` waits for ever.
    pub timeout: Option<f32>,
}

impl RebindRequest {
    pub fn new(player: PlayerId, action: ActionHandle, slot: u32) -> Self {
        Self {
            player,
            action,
            slot,
            part: None,
            devices: DeviceFilter::Player,
            cancel: vec![
                ControlRef::new(Device::Keyboard, "Escape"),
                ControlRef::new(Device::Gamepad, "Select"),
            ],
            conflicts: ConflictPolicy::Swap,
            timeout: Some(10.0),
        }
    }
}

/// Where a rebinding is.
#[derive(Clone, Debug, PartialEq)]
pub enum RebindState {
    Waiting,
    Bound {
        binding: Binding,
        /// The action (and slot) that received this slot's old binding.
        swapped: Option<(ActionHandle, u32)>,
    },
    Canceled,
    /// The control conflicts (the message names the other action).
    Refused(String),
    TimedOut,
}

struct Rebind {
    req: RebindRequest,
    state: RebindState,
    elapsed: f32,
}

pub(crate) struct Player {
    pub active: bool,
    pub devices: Vec<DeviceId>,
    /// Stable keys of devices lost while this player owned them.
    pub lost: Vec<String>,
    pub enabled: Vec<bool>,
    pub overrides: BindingOverrides,
    pub bindings: Vec<CBinding>,
    pub ranges: Vec<(u32, u32)>,
    pub states: Vec<ActionState>,
    pub results: Vec<TResult>,
    pub prev: Vec<TResult>,
    pub was: Vec<bool>,
    pub tstates: Vec<TState>,
    /// Per control: 0, or the index + 1 of the context that consumed it this frame
    /// (`u16::MAX`: suppressed for every context).
    pub consumed: [Vec<u16>; 4],
    /// Controls hidden until released (the one that just completed a rebinding).
    pub suppressed: Vec<(Device, u16)>,
}

impl Player {
    fn new() -> Self {
        Self {
            active: false,
            devices: Vec::new(),
            lost: Vec::new(),
            enabled: Vec::new(),
            overrides: BindingOverrides::default(),
            bindings: Vec::new(),
            ranges: Vec::new(),
            states: Vec::new(),
            results: Vec::new(),
            prev: Vec::new(),
            was: Vec::new(),
            tstates: Vec::new(),
            consumed: Device::ALL.map(|d| vec![0u16; control_count(d)]),
            suppressed: Vec::new(),
        }
    }

    /// Size every per-action table for `map` and compile the bindings with the overrides.
    fn rebuild(&mut self, map: &CompiledMap, faults: &InputFaults) {
        let n = map.actions.len();
        self.enabled.resize(map.contexts.len(), true);
        self.states = vec![ActionState::default(); n];
        self.results = vec![TResult::None; n];
        self.prev = vec![TResult::None; n];
        self.was = vec![false; n];
        self.tstates = vec![TState::default(); map.trigger_states as usize];
        self.recompile(map, faults);
    }

    fn recompile(&mut self, map: &CompiledMap, faults: &InputFaults) {
        self.bindings.clear();
        self.ranges.clear();
        for a in &map.actions {
            let start = u32::try_from(self.bindings.len()).unwrap_or(u32::MAX);
            for (slot, b) in effective(a.path.as_str(), &a.defaults, &self.overrides, faults) {
                if let Ok(c) = CBinding::compile(slot, &b, a.kind) {
                    self.bindings.push(c);
                }
            }
            let end = u32::try_from(self.bindings.len()).unwrap_or(u32::MAX);
            self.ranges.push((start, end));
        }
    }
}

/// An action's bindings with a player's overrides applied, by slot.
fn effective(
    path: &str,
    defaults: &BTreeMap<u32, BindingDef>,
    o: &BindingOverrides,
    faults: &InputFaults,
) -> Vec<(u32, BindingDef)> {
    let mut out: BTreeMap<u32, BindingDef> = defaults.clone();
    let _ = faults;
    if let Some(slots) = o.map.get(path) {
        for (slot, b) in slots {
            match b {
                None => {
                    out.remove(slot);
                }
                Some(b) => {
                    // An override keeps the default's modifiers (a stick deadzone stays on the
                    // stick the player picked); a new slot has none.
                    let mods = out
                        .get(slot)
                        .map(|d| d.modifiers.clone())
                        .unwrap_or_default();
                    out.insert(
                        *slot,
                        BindingDef {
                            binding: b.clone(),
                            modifiers: mods,
                        },
                    );
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Counters (the debugger and the budgets read them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputStats {
    pub updates: u64,
    pub events: u64,
    pub bindings_read: u64,
    pub actions_evaluated: u64,
}

/// One logged raw event (the debugger's event list; recorded only while the log is on).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoggedEvent {
    pub frame: u64,
    pub device: DeviceId,
    pub control: u16,
    pub value: [f32; 2],
}

/// The input runtime (see the module docs).
pub struct InputRuntime {
    pub(crate) map: CompiledMap,
    pub(crate) has_map: bool,
    pub(crate) devices: Vec<DeviceSlot>,
    queue: Vec<RawEvent>,
    deferred: Vec<RawEvent>,
    touches: Vec<(DeviceId, TouchInput)>,
    motions: Vec<(DeviceId, MotionSample)>,
    actuations: Vec<(DeviceId, u16)>,
    pub(crate) players: Vec<Player>,
    policy: JoinPolicy,
    player_events: Vec<PlayerEvent>,
    events: Vec<ActionEvent>,
    rebind: Option<Rebind>,
    pub(crate) haptics: HapticsState,
    pub(crate) frame: u64,
    pub(crate) now: f64,
    pub(crate) stats: InputStats,
    pub(crate) log: Option<(usize, std::collections::VecDeque<LoggedEvent>)>,
    gesture_cfg: GestureConfig,
    gyro_cfg: GyroSettings,
    faults: InputFaults,
    tracer: Option<&'static forge_trace::Tracer>,
    scratch_alloc: Vec<Vec<u8>>,
}

impl Default for InputRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl InputRuntime {
    pub fn new() -> Self {
        Self {
            map: CompiledMap::default(),
            has_map: false,
            devices: Vec::new(),
            queue: Vec::with_capacity(256),
            deferred: Vec::new(),
            touches: Vec::new(),
            motions: Vec::new(),
            actuations: Vec::with_capacity(16),
            players: Vec::new(),
            policy: JoinPolicy::Single,
            player_events: Vec::with_capacity(8),
            events: Vec::with_capacity(64),
            rebind: None,
            haptics: HapticsState::default(),
            frame: 0,
            now: 0.0,
            stats: InputStats::default(),
            log: None,
            gesture_cfg: GestureConfig::default(),
            gyro_cfg: GyroSettings::default(),
            faults: InputFaults::default(),
            tracer: None,
            scratch_alloc: Vec::new(),
        }
    }

    /// Report `input.update` as a zone of `tracer` (the profiler's input budget row).
    pub fn with_tracer(mut self, tracer: &'static forge_trace::Tracer) -> Self {
        self.tracer = Some(tracer);
        self
    }

    pub fn with_faults(mut self, f: InputFaults) -> Self {
        self.faults = f;
        self
    }

    pub fn faults(&self) -> &InputFaults {
        &self.faults
    }

    // ---- the map ----------------------------------------------------------------------------

    /// Compile and use `def` (players keep their overrides; state restarts). Returns what
    /// could not be compiled.
    pub fn set_map(&mut self, def: &InputMapDef) -> Vec<String> {
        self.map = CompiledMap::compile(def);
        self.has_map = true;
        let (map, faults) = (&self.map, &self.faults);
        for p in &mut self.players {
            p.enabled.clear();
            p.rebuild(map, faults);
        }
        self.events.reserve(self.map.actions.len() * 2);
        if self.policy == JoinPolicy::Single && self.players.is_empty() {
            self.ensure_single_player();
        }
        self.map.problems.clone()
    }

    pub fn map(&self) -> &CompiledMap {
        &self.map
    }

    pub fn handle(&self, path: &str) -> Option<ActionHandle> {
        self.map.handle(path)
    }

    // ---- devices ----------------------------------------------------------------------------

    /// A device connected. A device that comes back with a stable key it had before reuses
    /// its id and returns to the player that lost it.
    pub fn connect(&mut self, desc: DeviceDesc) -> DeviceId {
        let back = self
            .devices
            .iter()
            .position(|d| !d.connected && d.desc.stable_key == desc.stable_key);
        let id = match back {
            Some(i) => {
                let slot = DeviceSlot::new(desc);
                self.devices[i] = slot;
                DeviceId(u32::try_from(i).unwrap_or(u32::MAX))
            }
            None => {
                self.devices.push(DeviceSlot::new(desc));
                DeviceId(u32::try_from(self.devices.len() - 1).unwrap_or(u32::MAX))
            }
        };
        let key = self.devices[id.0 as usize].desc.stable_key.clone();
        // Back to the player that lost it.
        if !self.faults.no_reconnect_pairing()
            && let Some(p) = self.players.iter().position(|p| p.lost.contains(&key))
        {
            self.players[p].lost.retain(|k| *k != key);
            self.give(id, p);
            self.player_events.push(PlayerEvent::DeviceRegained {
                player: PlayerId(u8::try_from(p).unwrap_or(u8::MAX)),
                device: id,
            });
            return id;
        }
        if self.policy == JoinPolicy::Single {
            self.ensure_single_player();
            self.give(id, 0);
        }
        id
    }

    /// A device went away: its controls read zero, and its player remembers it (a reconnect
    /// goes back to that player; the game can show a "reconnect your controller" prompt from
    /// [`PlayerEvent::DeviceLost`]).
    pub fn disconnect(&mut self, id: DeviceId) {
        let Some(slot) = self.devices.get_mut(id.0 as usize) else {
            return;
        };
        if !slot.connected {
            return;
        }
        slot.connected = false;
        slot.values.iter_mut().for_each(|v| *v = [0.0; 2]);
        let key = slot.desc.stable_key.clone();
        if let Some(p) = slot.player.take() {
            let pl = &mut self.players[usize::from(p)];
            pl.devices.retain(|d| *d != id);
            pl.lost.push(key);
            self.player_events.push(PlayerEvent::DeviceLost {
                player: PlayerId(p),
                device: id,
            });
        }
        self.haptics.forget(id);
    }

    /// Every device the runtime has seen, connected or not.
    pub fn devices(&self) -> impl Iterator<Item = (DeviceId, &DeviceDesc, bool)> + '_ {
        self.devices.iter().enumerate().map(|(i, d)| {
            (
                DeviceId(u32::try_from(i).unwrap_or(u32::MAX)),
                &d.desc,
                d.connected,
            )
        })
    }

    pub fn device(&self, id: DeviceId) -> Option<&DeviceDesc> {
        self.devices.get(id.0 as usize).map(|d| &d.desc)
    }

    /// A control's current value on a device.
    pub fn control_value(&self, id: DeviceId, control: u16) -> [f32; 2] {
        self.devices
            .get(id.0 as usize)
            .and_then(|d| d.values.get(usize::from(control)))
            .copied()
            .unwrap_or([0.0; 2])
    }

    /// The player owning a device.
    pub fn owner(&self, id: DeviceId) -> Option<PlayerId> {
        self.devices
            .get(id.0 as usize)
            .and_then(|d| d.player)
            .map(PlayerId)
    }

    // ---- raw input ---------------------------------------------------------------------------

    /// Queue a raw event (applied by the next [`InputRuntime::update`]).
    pub fn push(&mut self, ev: RawEvent) {
        self.queue.push(ev);
    }

    /// Queue a change of the named control (a virtual device's, a test's). Unknown names
    /// are ignored.
    pub fn send(&mut self, device: DeviceId, control: &str, value: [f32; 2]) {
        let Some(class) = self.devices.get(device.0 as usize).map(|d| d.desc.class) else {
            return;
        };
        if let Some(i) = control_index(class, control) {
            self.push(RawEvent {
                device,
                control: i,
                value,
            });
        }
    }

    /// Queue a button press and release in the same frame (a tap faster than a frame).
    pub fn tap(&mut self, device: DeviceId, control: &str) {
        self.send(device, control, [1.0, 0.0]);
        self.send(device, control, [0.0, 0.0]);
    }

    /// Release every control of a device (a window lost focus: no stuck keys).
    pub fn release_all(&mut self, device: DeviceId) {
        let Some(d) = self.devices.get(device.0 as usize) else {
            return;
        };
        for (i, v) in d.values.iter().enumerate() {
            if *v != [0.0; 2]
                && let Ok(i) = u16::try_from(i)
                && control_behaviour(d.desc.class, i) == Behaviour::State
            {
                self.queue.push(RawEvent {
                    device,
                    control: i,
                    value: [0.0; 2],
                });
            }
        }
    }

    /// Queue a touch (a touch-class device's fingers; gestures are recognised in
    /// [`InputRuntime::update`]).
    pub fn touch(&mut self, device: DeviceId, t: TouchInput) {
        self.touches.push((device, t));
    }

    /// Queue a motion sample (gyro and accelerometer) for a gamepad.
    pub fn motion(&mut self, device: DeviceId, s: MotionSample) {
        self.motions.push((device, s));
    }

    pub fn set_gesture_config(&mut self, c: GestureConfig) {
        self.gesture_cfg = c;
    }

    pub fn set_gyro_settings(&mut self, s: GyroSettings) {
        self.gyro_cfg = s;
        for d in &mut self.devices {
            if let Some(g) = &mut d.gyro {
                g.settings = s;
            }
        }
    }

    // ---- players -----------------------------------------------------------------------------

    pub fn set_join_policy(&mut self, p: JoinPolicy) {
        self.policy = p;
        if self.policy == JoinPolicy::Single {
            self.ensure_single_player();
            for i in 0..self.devices.len() {
                if self.devices[i].connected && self.devices[i].player.is_none() {
                    self.give(DeviceId(u32::try_from(i).unwrap_or(u32::MAX)), 0);
                }
            }
        }
    }

    pub fn join_policy(&self) -> &JoinPolicy {
        &self.policy
    }

    fn ensure_single_player(&mut self) {
        if self.players.is_empty() {
            self.add_player();
        } else if let Some(p) = self.players.first_mut() {
            p.active = true;
        }
    }

    /// Activate the next free player.
    pub fn add_player(&mut self) -> Option<PlayerId> {
        let i = match self.players.iter().position(|p| !p.active) {
            Some(i) => i,
            None if self.players.len() < MAX_PLAYERS => {
                self.players.push(Player::new());
                self.players.len() - 1
            }
            None => return None,
        };
        let (map, faults) = (&self.map, &self.faults);
        let p = &mut self.players[i];
        p.active = true;
        p.enabled.clear();
        p.rebuild(map, faults);
        Some(PlayerId(u8::try_from(i).unwrap_or(u8::MAX)))
    }

    /// Deactivate a player; its devices become free.
    pub fn remove_player(&mut self, id: PlayerId) {
        let Some(p) = self.players.get_mut(usize::from(id.0)) else {
            return;
        };
        p.active = false;
        let devs = std::mem::take(&mut p.devices);
        p.lost.clear();
        for d in devs {
            if let Some(s) = self.devices.get_mut(d.0 as usize) {
                s.player = None;
            }
        }
        self.player_events.push(PlayerEvent::Left { player: id });
    }

    /// Give a device to a player (taking it from any other).
    pub fn assign(&mut self, device: DeviceId, player: PlayerId) {
        let p = usize::from(player.0);
        while self.players.len() <= p && self.players.len() < MAX_PLAYERS {
            self.players.push(Player::new());
        }
        if let Some(pl) = self.players.get_mut(p)
            && !pl.active
        {
            pl.active = true;
            pl.enabled.clear();
            pl.rebuild(&self.map, &self.faults);
        }
        self.give(device, p);
    }

    fn give(&mut self, device: DeviceId, p: usize) {
        let Some(slot) = self.devices.get_mut(device.0 as usize) else {
            return;
        };
        if let Some(old) = slot.player.take()
            && let Some(o) = self.players.get_mut(usize::from(old))
        {
            o.devices.retain(|d| *d != device);
        }
        slot.player = u8::try_from(p).ok();
        if let Some(pl) = self.players.get_mut(p)
            && !pl.devices.contains(&device)
        {
            pl.devices.push(device);
        }
    }

    /// Active players.
    pub fn players(&self) -> impl Iterator<Item = PlayerId> + '_ {
        self.players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.active)
            .filter_map(|(i, _)| u8::try_from(i).ok().map(PlayerId))
    }

    /// A player's devices.
    pub fn player_devices(&self, p: PlayerId) -> &[DeviceId] {
        self.players
            .get(usize::from(p.0))
            .map_or(&[], |p| p.devices.as_slice())
    }

    /// Enable or disable a context for a player (each player has its own set: one player in
    /// a menu, another still driving).
    pub fn set_context_enabled(&mut self, p: PlayerId, c: ContextHandle, on: bool) {
        if let Some(e) = self
            .players
            .get_mut(usize::from(p.0))
            .and_then(|p| p.enabled.get_mut(usize::from(c.0)))
        {
            *e = on;
        }
    }

    pub fn context_enabled(&self, p: PlayerId, c: ContextHandle) -> bool {
        self.players
            .get(usize::from(p.0))
            .and_then(|p| p.enabled.get(usize::from(c.0)))
            .copied()
            .unwrap_or(false)
    }

    /// Player joins, losses and regains this frame.
    pub fn player_events(&self) -> &[PlayerEvent] {
        &self.player_events
    }

    // ---- actions -----------------------------------------------------------------------------

    /// An action's state for a player this frame.
    pub fn action(&self, p: PlayerId, a: ActionHandle) -> ActionState {
        self.players
            .get(usize::from(p.0))
            .and_then(|p| p.states.get(a.0 as usize))
            .copied()
            .unwrap_or_default()
    }

    /// The action events of this frame, every player.
    pub fn events(&self) -> &[ActionEvent] {
        &self.events
    }

    // ---- rebinding and overrides ---------------------------------------------------------------

    /// An action's bindings for a player (defaults with the player's overrides), by slot.
    pub fn bindings(&self, p: PlayerId, a: ActionHandle) -> Vec<(u32, BindingDef)> {
        let (Some(pl), Some(act)) = (
            self.players.get(usize::from(p.0)),
            self.map.actions.get(a.0 as usize),
        ) else {
            return Vec::new();
        };
        effective(&act.path, &act.defaults, &pl.overrides, &self.faults)
    }

    pub fn overrides(&self, p: PlayerId) -> Option<&BindingOverrides> {
        self.players.get(usize::from(p.0)).map(|p| &p.overrides)
    }

    /// Replace a player's overrides (loaded from the player's profile).
    pub fn set_overrides(&mut self, p: PlayerId, o: BindingOverrides) {
        let (map, faults) = (&self.map, &self.faults);
        if let Some(pl) = self.players.get_mut(usize::from(p.0)) {
            if faults.ignore_loaded_overrides() {
                return;
            }
            pl.overrides = o;
            pl.recompile(map, faults);
        }
    }

    /// Override one slot directly (a settings screen that picks from a list).
    pub fn set_binding(&mut self, p: PlayerId, a: ActionHandle, slot: u32, b: Option<Binding>) {
        let Some(path) = self.map.actions.get(a.0 as usize).map(|x| x.path.clone()) else {
            return;
        };
        let (map, faults) = (&self.map, &self.faults);
        if let Some(pl) = self.players.get_mut(usize::from(p.0)) {
            pl.overrides.set(&path, slot, b);
            pl.recompile(map, faults);
        }
    }

    /// Back to the authored bindings (one action, or all).
    pub fn reset_bindings(&mut self, p: PlayerId, a: Option<ActionHandle>) {
        let path = a.and_then(|a| self.map.actions.get(a.0 as usize).map(|x| x.path.clone()));
        let (map, faults) = (&self.map, &self.faults);
        if let Some(pl) = self.players.get_mut(usize::from(p.0)) {
            match path {
                Some(path) => pl.overrides.clear_action(&path),
                None => pl.overrides = BindingOverrides::default(),
            }
            pl.recompile(map, faults);
        }
    }

    /// Start listening for the control to bind (the next actuated control that fits).
    pub fn start_rebind(&mut self, req: RebindRequest) {
        self.rebind = Some(Rebind {
            req,
            state: RebindState::Waiting,
            elapsed: 0.0,
        });
    }

    pub fn cancel_rebind(&mut self) {
        if let Some(r) = &mut self.rebind
            && r.state == RebindState::Waiting
        {
            r.state = RebindState::Canceled;
        }
    }

    pub fn rebind_state(&self) -> Option<&RebindState> {
        self.rebind.as_ref().map(|r| &r.state)
    }

    pub fn rebind_request(&self) -> Option<&RebindRequest> {
        self.rebind.as_ref().map(|r| &r.req)
    }

    /// Take a finished rebinding's outcome (`None` while waiting or when none runs).
    pub fn take_rebind(&mut self) -> Option<RebindState> {
        if self
            .rebind
            .as_ref()
            .is_some_and(|r| r.state != RebindState::Waiting)
        {
            return self.rebind.take().map(|r| r.state);
        }
        None
    }

    // ---- haptics --------------------------------------------------------------------------------

    /// Rumble every gamepad of a player.
    pub fn rumble(&mut self, p: PlayerId, r: Rumble) {
        let devs: Vec<DeviceId> = self.player_devices(p).to_vec();
        for d in devs {
            if self
                .devices
                .get(d.0 as usize)
                .is_some_and(|s| s.desc.class == Device::Gamepad)
            {
                self.haptics.start(d, r, self.now);
            }
        }
    }

    pub fn rumble_device(&mut self, d: DeviceId, r: Rumble) {
        self.haptics.start(d, r, self.now);
    }

    pub fn stop_rumble(&mut self, p: PlayerId) {
        let devs: Vec<DeviceId> = self.player_devices(p).to_vec();
        for d in devs {
            self.haptics.stop(d);
        }
    }

    /// Motor levels that changed this frame (a backend applies them).
    pub fn motor_commands(&self) -> &[MotorCommand] {
        self.haptics.commands()
    }

    // ---- debugger ---------------------------------------------------------------------------------

    /// Keep the last `capacity` raw events for the debugger (0 turns the log off; off costs
    /// nothing).
    pub fn set_event_log(&mut self, capacity: usize) {
        self.log = (capacity > 0).then(|| (capacity, std::collections::VecDeque::new()));
    }

    pub fn stats(&self) -> InputStats {
        self.stats
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Seconds of input time (the sum of the updates' `dt`).
    pub fn now(&self) -> f64 {
        self.now
    }

    // ---- the frame --------------------------------------------------------------------------------

    /// Apply the events pushed since the last update and evaluate every player (see the module
    /// docs). `dt` is the seconds since the previous update.
    pub fn update(&mut self, dt: f32) {
        let _z = self.tracer.map(|t| t.zone("input.update"));
        self.frame += 1;
        self.now += f64::from(dt);
        self.stats.updates += 1;
        self.player_events.clear();
        self.events.clear();
        self.actuations.clear();
        if self.faults.alloc_per_frame() {
            self.scratch_alloc.clear();
            self.scratch_alloc.push(vec![0u8; 64]);
        }
        let idle = self.queue.is_empty()
            && self.deferred.is_empty()
            && self.touches.is_empty()
            && self.motions.is_empty()
            && self.devices.is_empty()
            && !self.faults.eval_without_players();
        if idle {
            return;
        }
        self.begin_frame();
        self.apply_events();
        self.apply_touches_and_motion(dt);
        self.auto_join();
        self.run_rebind(dt);
        self.haptics.update(self.now, &self.faults);
        if !self.has_map && !self.faults.eval_without_players() {
            return;
        }
        let InputRuntime {
            map,
            devices,
            players,
            faults,
            stats,
            events,
            ..
        } = self;
        let mut any = false;
        for (pi, p) in players.iter_mut().enumerate() {
            if !p.active {
                continue;
            }
            any = true;
            eval_player(map, devices, p, pi, dt, faults, stats, events);
        }
        if !any && faults.eval_without_players() {
            let mut ghost = Player::new();
            ghost.rebuild(map, faults);
            eval_player(map, devices, &mut ghost, 0, dt, faults, stats, events);
        }
    }

    /// Reset last frame's deltas and edges.
    fn begin_frame(&mut self) {
        for d in &mut self.devices {
            d.pressed.iter_mut().for_each(|w| *w = 0);
            d.released.iter_mut().for_each(|w| *w = 0);
            for &i in &d.deltas {
                if let Some(v) = d.values.get_mut(usize::from(i)) {
                    *v = [0.0; 2];
                }
            }
        }
    }

    fn apply_events(&mut self) {
        if self.faults.defer_events() {
            std::mem::swap(&mut self.queue, &mut self.deferred);
        }
        let frame = self.frame;
        for k in 0..self.queue.len() {
            let ev = self.queue[k];
            let Some(d) = self.devices.get_mut(ev.device.0 as usize) else {
                continue;
            };
            if !d.connected {
                continue;
            }
            let Some(v) = d.values.get_mut(usize::from(ev.control)) else {
                continue;
            };
            self.stats.events += 1;
            d.last_active = frame;
            let class = d.desc.class;
            match control_behaviour(class, ev.control) {
                Behaviour::Delta => {
                    v[0] += ev.value[0];
                    v[1] += ev.value[1];
                }
                Behaviour::Position => *v = ev.value,
                Behaviour::State => {
                    let old = mag(*v);
                    *v = ev.value;
                    let new = mag(ev.value);
                    if old < PRESS_POINT && new >= PRESS_POINT {
                        DeviceSlot::set_bit(&mut d.pressed, ev.control);
                        self.actuations.push((ev.device, ev.control));
                    } else if old >= PRESS_POINT && new < PRESS_POINT {
                        DeviceSlot::set_bit(&mut d.released, ev.control);
                    }
                }
            }
            if let Some((cap, log)) = &mut self.log {
                if log.len() >= *cap {
                    log.pop_front();
                }
                log.push_back(LoggedEvent {
                    frame,
                    device: ev.device,
                    control: ev.control,
                    value: ev.value,
                });
            }
        }
        self.queue.clear();
    }

    fn apply_touches_and_motion(&mut self, dt: f32) {
        let now = self.now;
        let cfg = self.gesture_cfg;
        for k in 0..self.touches.len() {
            let (dev, t) = self.touches[k];
            if let Some(d) = self.devices.get_mut(dev.0 as usize)
                && d.connected
                && let Some(g) = &mut d.gestures
            {
                d.last_active = self.frame;
                g.touch(t, now, &cfg, &self.faults);
            }
        }
        self.touches.clear();
        // Gestures write their controls every frame a touch device has fingers or pulses.
        for (di, d) in self.devices.iter_mut().enumerate() {
            let Some(g) = &mut d.gestures else { continue };
            let out = g.frame(now, &cfg, &self.faults);
            if out.idle {
                continue;
            }
            let set = |name: &str, v: [f32; 2], d: &mut DeviceSlot| {
                if let Some(i) = control_index(Device::Touch, name)
                    && let Some(x) = d.values.get_mut(usize::from(i))
                {
                    *x = v;
                }
            };
            let primary = if out.fingers > 0 { 1.0 } else { 0.0 };
            let pi = control_index(Device::Touch, "Primary").unwrap_or(0);
            let was = d.values.get(usize::from(pi)).map_or(0.0, |v| v[0]);
            if was < PRESS_POINT && primary >= PRESS_POINT {
                DeviceSlot::set_bit(&mut d.pressed, pi);
                self.actuations
                    .push((DeviceId(u32::try_from(di).unwrap_or(u32::MAX)), pi));
            } else if was >= PRESS_POINT && primary < PRESS_POINT {
                DeviceSlot::set_bit(&mut d.released, pi);
            }
            set("Primary", [primary, 0.0], d);
            set("Position", out.position, d);
            set("Delta", out.delta, d);
            set("Pinch", [out.pinch, 0.0], d);
            set("Rotate", [out.rotate, 0.0], d);
            set("TwoFingerPan", out.pan, d);
            for name in out.pulses.names() {
                if let Some(i) = control_index(Device::Touch, name) {
                    // A pulse: pressed for this frame only.
                    DeviceSlot::set_bit(&mut d.pressed, i);
                    DeviceSlot::set_bit(&mut d.released, i);
                    self.actuations
                        .push((DeviceId(u32::try_from(di).unwrap_or(u32::MAX)), i));
                }
            }
        }
        let gyro_i = control_index(Device::Gamepad, "Gyro").unwrap_or(0);
        for k in 0..self.motions.len() {
            let (dev, s) = self.motions[k];
            let Some(d) = self.devices.get_mut(dev.0 as usize) else {
                continue;
            };
            if !d.connected || d.desc.class != Device::Gamepad {
                continue;
            }
            let settings = self.gyro_cfg;
            let g = d.gyro.get_or_insert_with(|| GyroProcessor::new(settings));
            let out = g.process(&s, &self.faults);
            if let Some(v) = d.values.get_mut(usize::from(gyro_i)) {
                v[0] += out[0];
                v[1] += out[1];
            }
        }
        self.motions.clear();
        let _ = dt;
    }

    fn auto_join(&mut self) {
        let JoinPolicy::AutoJoin {
            max_players,
            button,
        } = self.policy.clone()
        else {
            return;
        };
        for k in 0..self.actuations.len() {
            let (dev, ctl) = self.actuations[k];
            let Some(d) = self.devices.get(dev.0 as usize) else {
                continue;
            };
            if d.player.is_some() && !self.faults.join_assigned_devices() {
                continue;
            }
            let class = d.desc.class;
            if control_shape(class, ctl) != Shape::Button {
                continue;
            }
            if let Some(b) = &button
                && (b.device != class || control_index(class, &b.name) != Some(ctl))
            {
                continue;
            }
            let active = self.players.iter().filter(|p| p.active).count();
            if active >= usize::from(max_players) {
                continue;
            }
            let Some(p) = self.add_player() else { continue };
            self.give(dev, usize::from(p.0));
            // A keyboard and the mice are one seat.
            if matches!(class, Device::Keyboard | Device::Mouse) {
                let partner = if class == Device::Keyboard {
                    Device::Mouse
                } else {
                    Device::Keyboard
                };
                for i in 0..self.devices.len() {
                    let s = &self.devices[i];
                    if s.connected && s.desc.class == partner && s.player.is_none() {
                        self.give(
                            DeviceId(u32::try_from(i).unwrap_or(u32::MAX)),
                            usize::from(p.0),
                        );
                    }
                }
            }
            self.player_events.push(PlayerEvent::Joined {
                player: p,
                device: dev,
            });
        }
    }

    fn run_rebind(&mut self, dt: f32) {
        let Some(r) = &mut self.rebind else { return };
        if r.state != RebindState::Waiting {
            return;
        }
        r.elapsed += dt;
        let req = r.req.clone();
        let Some(act) = self.map.actions.get(req.action.0 as usize) else {
            r.state = RebindState::Refused("no such action".into());
            return;
        };
        let Some(pl) = self.players.get(usize::from(req.player.0)) else {
            r.state = RebindState::Refused("no such player".into());
            return;
        };
        let mut found: Option<(Device, u16)> = None;
        for &(dev, ctl) in &self.actuations {
            if !pl.devices.contains(&dev) {
                continue;
            }
            let Some(d) = self.devices.get(dev.0 as usize) else {
                continue;
            };
            let class = d.desc.class;
            if let DeviceFilter::Classes(cs) = &req.devices
                && !cs.contains(&class)
            {
                continue;
            }
            let cref = ControlRef::new(class, control_name(class, ctl));
            if req.cancel.contains(&cref) {
                r.state = RebindState::Canceled;
                return;
            }
            let shape = control_shape(class, ctl);
            let fits = self.faults.rebind_any_shape()
                || match (req.part, act.kind) {
                    (Some(_), _) => shape == Shape::Button,
                    (None, ActionKind::Button) => shape == Shape::Button,
                    (None, ActionKind::Axis1D) => {
                        matches!(shape, Shape::Axis1D | Shape::Button)
                    }
                    (None, ActionKind::Axis2D) => shape == Shape::Axis2D,
                };
            if fits {
                found = Some((class, ctl));
                break;
            }
        }
        let Some((class, ctl)) = found else {
            if req.timeout.is_some_and(|t| r.elapsed >= t) {
                r.state = RebindState::TimedOut;
            }
            return;
        };
        let cref = ControlRef::new(class, control_name(class, ctl));
        let current = effective(&act.path, &act.defaults, &pl.overrides, &self.faults);
        let old = current
            .iter()
            .find(|(s, _)| *s == req.slot)
            .map(|(_, b)| b.binding.clone());
        let new = match (req.part, &old) {
            (None, _) => Binding::Control(cref.clone()),
            (Some(k), Some(Binding::Composite2D(parts))) if k < 4 => {
                let mut p = parts.clone();
                p[usize::from(k)] = cref.clone();
                Binding::Composite2D(p)
            }
            (Some(k), Some(Binding::Composite1D(a, b))) if k < 2 => {
                if k == 0 {
                    Binding::Composite1D(cref.clone(), b.clone())
                } else {
                    Binding::Composite1D(a.clone(), cref.clone())
                }
            }
            (Some(_), _) => {
                r.state = RebindState::Refused(format!(
                    "slot {} of {} is not a composite",
                    req.slot, act.name
                ));
                return;
            }
        };
        // Conflicts: another action of the same context using the control.
        let mut conflict: Option<(u32, u32, String)> = None;
        for (ai, other) in self.map.actions.iter().enumerate() {
            let ai = u32::try_from(ai).unwrap_or(u32::MAX);
            if ai == req.action.0 || other.ctx != act.ctx {
                continue;
            }
            for (slot, b) in effective(&other.path, &other.defaults, &pl.overrides, &self.faults) {
                if b.binding.controls().contains(&&cref) {
                    conflict = Some((ai, slot, other.name.clone()));
                    break;
                }
            }
            if conflict.is_some() {
                break;
            }
        }
        let mut swapped = None;
        let path = act.path.clone();
        if let Some((oa, oslot, oname)) = conflict {
            match req.conflicts {
                ConflictPolicy::Refuse => {
                    r.state = RebindState::Refused(format!(
                        "{cref} is already bound to \u{201c}{oname}\u{201d}"
                    ));
                    return;
                }
                ConflictPolicy::Swap => {
                    let other_path = self.map.actions[oa as usize].path.clone();
                    let pl = &mut self.players[usize::from(req.player.0)];
                    pl.overrides.set(&other_path, oslot, old.clone());
                    swapped = Some((ActionHandle(oa), oslot));
                }
                ConflictPolicy::Allow => {}
            }
        }
        let (map, faults) = (&self.map, &self.faults);
        let pl = &mut self.players[usize::from(req.player.0)];
        pl.overrides.set(&path, req.slot, Some(new.clone()));
        pl.recompile(map, faults);
        // The control that completed the rebinding does not also fire its new action.
        pl.suppressed.push((class, ctl));
        r.state = RebindState::Bound {
            binding: new,
            swapped,
        };
    }

    /// The controls actuated this frame (what a "press to bind" in the editor captures).
    pub fn actuated(&self) -> &[(DeviceId, u16)] {
        &self.actuations
    }
}

struct Frame<'a> {
    results: &'a [TResult],
    prev: &'a [TResult],
}

impl Others for Frame<'_> {
    fn result(&self, a: u32) -> TResult {
        self.results.get(a as usize).copied().unwrap_or_default()
    }
    fn fresh(&self, a: u32) -> bool {
        self.result(a) == TResult::Triggered
            && self.prev.get(a as usize).copied().unwrap_or_default() != TResult::Triggered
    }
}

#[allow(clippy::too_many_arguments)]
fn eval_player(
    map: &CompiledMap,
    devices: &[DeviceSlot],
    p: &mut Player,
    pi: usize,
    dt: f32,
    faults: &InputFaults,
    stats: &mut InputStats,
    events: &mut Vec<ActionEvent>,
) {
    let Player {
        devices: mine,
        enabled,
        bindings,
        ranges,
        states,
        results,
        prev,
        was,
        tstates,
        consumed,
        suppressed,
        ..
    } = p;
    let player = PlayerId(u8::try_from(pi).unwrap_or(u8::MAX));
    for c in consumed.iter_mut() {
        c.iter_mut().for_each(|w| *w = 0);
    }
    // Controls suppressed after a rebinding stay hidden until released.
    suppressed.retain(|&(class, i)| {
        let held = mine.iter().any(|d| {
            devices.get(d.0 as usize).is_some_and(|s| {
                s.desc.class == class && mag(s.values[usize::from(i)]) >= PRESS_POINT
            })
        });
        if held && let Some(c) = consumed[class.index()].get_mut(usize::from(i)) {
            *c = u16::MAX;
        }
        held
    });
    prev.copy_from_slice(results);
    results.iter_mut().for_each(|r| *r = TResult::None);
    for &a in &map.order {
        let ai = a as usize;
        let Some(act) = map.actions.get(ai) else {
            continue;
        };
        stats.actions_evaluated += 1;
        let on = enabled.get(usize::from(act.ctx)).copied().unwrap_or(false);
        let prio = map
            .contexts
            .get(usize::from(act.ctx))
            .map_or(0, |c| c.priority);
        let mut value = [0.0f32; 2];
        if on {
            let (s, e) = ranges.get(ai).copied().unwrap_or((0, 0));
            let mut best = 0.0f32;
            for b in &bindings[s as usize..e as usize] {
                stats.bindings_read += 1;
                let mut v = read(
                    devices,
                    mine,
                    consumed,
                    &map.contexts,
                    prio,
                    &b.source,
                    faults,
                );
                for m in &b.modifiers {
                    v = m.apply(v, b.shape, faults);
                }
                let mv = mag(v);
                if mv > best {
                    best = mv;
                    value = v;
                }
            }
            let shape = match act.kind {
                ActionKind::Button => Shape::Button,
                ActionKind::Axis1D => Shape::Axis1D,
                ActionKind::Axis2D => Shape::Axis2D,
            };
            for m in &act.modifiers {
                value = m.apply(value, shape, faults);
            }
            if act.kind != ActionKind::Axis2D {
                value[1] = 0.0;
            }
        }
        let m = mag(value);
        let actuated = on
            && match act.kind {
                ActionKind::Button => m >= PRESS_POINT,
                _ => m > AXIS_ACTUATION,
            };
        let tin = TIn {
            actuated,
            was: was[ai],
            dt,
        };
        let mut explicit = TResult::None;
        let mut implicit_ok = true;
        let base = act.tstate as usize;
        let res = if on {
            let fr = Frame {
                results: results.as_slice(),
                prev: prev.as_slice(),
            };
            for (k, t) in act.triggers.iter().enumerate() {
                let st = &mut tstates[base + k];
                let r = trigger::step(t, st, &tin, &fr, faults);
                if t.implicit() {
                    implicit_ok &= r == TResult::Triggered;
                } else {
                    explicit = explicit.max(r);
                }
            }
            trigger::combine(explicit, act.any_explicit, implicit_ok)
        } else {
            for k in 0..act.triggers.len() {
                tstates[base + k] = TState::default();
            }
            TResult::None
        };
        results[ai] = res;
        was[ai] = actuated;
        // Consumption: an actuated action of a consuming context hides its controls from
        // the contexts evaluated after it.
        if on
            && res != TResult::None
            && !faults.no_consumption()
            && map
                .contexts
                .get(usize::from(act.ctx))
                .is_some_and(|c| c.consume)
        {
            let (s, e) = ranges.get(ai).copied().unwrap_or((0, 0));
            for b in &bindings[s as usize..e as usize] {
                b.source.for_each(|d, i| {
                    if let Some(c) = consumed[d.index()].get_mut(usize::from(i))
                        && *c == 0
                    {
                        *c = act.ctx + 1;
                    }
                });
            }
        }
        // Phase events.
        let before = prev[ai];
        let st = &mut states[ai];
        st.value = value;
        st.started = before == TResult::None && res != TResult::None;
        st.triggered = res == TResult::Triggered;
        st.completed = before == TResult::Triggered && res == TResult::None;
        st.canceled = before == TResult::Ongoing && res == TResult::None;
        st.phase = match res {
            TResult::None => Phase::Idle,
            TResult::Ongoing => Phase::Ongoing,
            TResult::Triggered => Phase::Triggered,
        };
        if res == TResult::None {
            st.elapsed = 0.0;
        } else if !st.started {
            st.elapsed += dt;
        }
        if res == TResult::Triggered && before != TResult::Triggered {
            st.trigger_count += 1;
        }
        let action = ActionHandle(a);
        for (flag, kind) in [
            (st.started, ActionEventKind::Started),
            (st.triggered, ActionEventKind::Triggered),
            (st.completed, ActionEventKind::Completed),
            (st.canceled, ActionEventKind::Canceled),
        ] {
            if flag {
                events.push(ActionEvent {
                    player,
                    action,
                    kind,
                });
            }
        }
    }
}

/// A binding's raw value over the player's devices of its class (the largest magnitude).
fn read(
    devices: &[DeviceSlot],
    mine: &[DeviceId],
    consumed: &[Vec<u16>; 4],
    contexts: &[CContext],
    prio: i32,
    src: &Source,
    faults: &InputFaults,
) -> [f32; 2] {
    let one = |d: Device, i: u16| -> [f32; 2] {
        let i = if faults.lookup_by_name() {
            let path = format!("{}/{}", d.name(), control_name(d, i));
            ControlRef::parse(&path)
                .ok()
                .and_then(|c| c.index())
                .unwrap_or(i)
        } else {
            i
        };
        // Consumed by a context of higher priority (or suppressed): hidden from this one.
        let by = consumed[d.index()]
            .get(usize::from(i))
            .copied()
            .unwrap_or(0);
        if by == u16::MAX
            || (by > 0
                && contexts
                    .get(usize::from(by - 1))
                    .is_some_and(|c| c.priority > prio))
        {
            return [0.0; 2];
        }
        let mut best = [0.0f32; 2];
        let mut bm = 0.0f32;
        for id in mine {
            let Some(s) = devices.get(id.0 as usize) else {
                continue;
            };
            if s.desc.class != d || !s.connected {
                continue;
            }
            let mut v = s.values.get(usize::from(i)).copied().unwrap_or([0.0; 2]);
            if !faults.drop_same_frame_press()
                && DeviceSlot::bit(&s.pressed, i)
                && control_shape(d, i) == Shape::Button
            {
                v[0] = v[0].max(1.0);
            }
            let m = mag(v);
            if m > bm {
                bm = m;
                best = v;
            }
        }
        best
    };
    match src {
        Source::Control(d, i) => one(*d, *i),
        Source::Comp1([n, p]) => [one(p.0, p.1)[0] - one(n.0, n.1)[0], 0.0],
        Source::Comp2([u, dn, l, r]) => {
            let x = one(r.0, r.1)[0] - one(l.0, l.1)[0];
            let y = one(u.0, u.1)[0] - one(dn.0, dn.1)[0];
            let m = (x * x + y * y).sqrt();
            if m > 1.0 { [x / m, y / m] } else { [x, y] }
        }
    }
}

/// A binding modifier chain applied to one value (the debugger's per-binding view).
pub fn apply_chain(v: [f32; 2], shape: Shape, chain: &[Modifier]) -> [f32; 2] {
    let f = InputFaults::default();
    chain.iter().fold(v, |v, m| m.apply(v, shape, &f))
}
