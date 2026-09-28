//! W2 fault switches (ADR 0046): present only under `cfg(any(test, feature = "controls"))`.
//!
//! The editor's fault-switch sets (`CoreFaults`, `PanelFaults`, ...) are declared with
//! [`forge_trace::control_switches!`]; the single switches a tool or a field carries are
//! the types below. In a production build each is zero-sized, every accessor is a
//! constant, and the constructors that turn a fault on do not exist.

forge_trace::control_switches! {
    /// One W2 fault switch (see the [module docs](self)).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Switch {
        /// The fault is injected.
        pub on: bool,
    }
}
