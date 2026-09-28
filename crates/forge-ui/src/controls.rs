//! W2 fault switches (ADR 0046): present only under `cfg(any(test, feature = "controls"))`.
//!
//! [`Switch`] is one switch a widget carries for a guard's positive control. In a
//! production build it is zero-sized and [`Switch::on`] is the constant `false`, so the
//! branch it guards folds away; the `with_fault_*` builders that turn one on exist only in
//! test builds.

forge_trace::control_switches! {
    /// One W2 fault switch (see the [module docs](self)).
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Switch {
        /// The fault is injected.
        pub on: bool,
    }
}
