//! W2 fault switches (ADR 0046): each breaks one guarded property so its guard is seen to
//! fail. They exist only in test builds (`controls` feature, dev-dependencies only); in a
//! production build [`InputFaults`] is a zero-sized struct whose accessors are constant
//! `false`, so every branch on a switch folds away.

forge_trace::control_switches! {
    /// The input runtime's fault switches (see the module docs).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct InputFaults {
        /// Deadzones cut but never rescale (the classic jump past the deadzone).
        pub deadzone_no_rescale: bool,
        /// Radial deadzones are applied per axis (a diagonal is cut too early).
        pub radial_as_axial: bool,
        /// Curves are linear whatever they say.
        pub curve_linear: bool,
        /// Hold fires on the press, whatever its time.
        pub hold_ignores_time: bool,
        /// Tap fires on any release, however long the press.
        pub tap_ignores_max: bool,
        /// Multi-tap counts taps however far apart they are.
        pub multitap_ignores_gap: bool,
        /// Chords are not checked (the action fires without its chord).
        pub chord_ignored: bool,
        /// Combos advance on any step's action, in any order.
        pub combo_ignores_order: bool,
        /// Events pushed before an update are applied one update later (a frame of latency).
        pub defer_events: bool,
        /// A press and its release inside one frame leave no trace (a lost tap).
        pub drop_same_frame_press: bool,
        /// The map is evaluated even when no player can read it (the idle cost).
        pub eval_without_players: bool,
        /// Every update allocates a scratch buffer (the steady-state allocation guard).
        pub alloc_per_frame: bool,
        /// Higher-priority contexts do not consume their controls.
        pub no_consumption: bool,
        /// Loaded binding overrides are not applied.
        pub ignore_loaded_overrides: bool,
        /// A device that reconnects is not given back to its player.
        pub no_reconnect_pairing: bool,
        /// Auto-join also takes devices another player owns.
        pub join_assigned_devices: bool,
        /// Interactive rebinding accepts a control of any shape.
        pub rebind_any_shape: bool,
        /// Taps ignore finger movement (a drag becomes a tap).
        pub tap_no_slop: bool,
        /// The gyro is never calibrated (its bias drifts the aim).
        pub gyro_no_calibration: bool,
        /// A rumble outlives its duration.
        pub rumble_never_stops: bool,
        /// Controls are resolved from their `Device/Name` path on every read (a string built
        /// and searched per binding per frame: the regression the `input.update` budget
        /// exists to catch).
        pub lookup_by_name: bool,
        /// On-screen controls do not capture their finger: a thumb sliding off a stick or a
        /// button lets go of it.
        pub onscreen_no_capture: bool,
    }
}
