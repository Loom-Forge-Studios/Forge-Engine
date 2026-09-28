//! W2 fault switches that exist only in test builds (WP-U12, ADR 0046).
//!
//! Every guard has a positive control: a fault that breaks the guarded property, so the
//! guard is seen to fail. The switches that inject those faults are **not part of a
//! production build**. A crate declares a `controls` cargo feature (off by default) and
//! wraps each switch set in [`control_switches!`]: under `cfg(any(test, feature =
//! "controls"))` the set is a plain struct of `pub` fields, and in every other build it is a
//! zero-sized struct (no public field, so nothing can turn a switch on) whose accessors are constant `Default` values, so each branch on a
//! switch folds away and no production struct carries a switch.
//!
//! Only `[dev-dependencies]` may turn `controls` on (`cargo xtask layering` refuses a normal
//! dependency edge that enables it), so a release build of the editor, the runtime or a
//! game never links a control branch.
//!
//! Reads go through the accessor, `faults.skip_hidden()`, which compiles in both builds;
//! writes (`PanelFaults { skip_hidden: true, ..Default::default() }`) exist only in tests.

/// Declares a set of W2 fault switches (see the [module docs](self)).
///
/// ```
/// forge_trace::control_switches! {
///     /// Switches for the example guard.
///     #[derive(Clone, Copy, Debug, Default)]
///     pub struct ExampleFaults {
///         /// Skip the check (the control).
///         pub skip_check: bool,
///         /// Rows to enumerate eagerly (the control), `None` normally.
///         pub enumerate: Option<u64>,
///     }
/// }
/// let f = ExampleFaults::default();
/// assert!(!f.skip_check());
/// assert_eq!(f.enumerate(), None);
/// ```
///
/// The attributes (docs, `derive`s) apply to both builds, so a derive must hold for a unit
/// struct too; `Default` is required. Every field type must be `Clone + Default`; an
/// accessor returns a clone of the field (a constant `Default` in a production build).
#[macro_export]
macro_rules! control_switches {
    (
        $(#[$m:meta])*
        $vis:vis struct $name:ident {
            $( $(#[$fm:meta])* pub $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$m])*
        #[cfg(any(test, feature = "controls"))]
        $vis struct $name {
            $( $(#[$fm])* pub $field: $ty, )*
        }

        #[cfg(any(test, feature = "controls"))]
        #[allow(dead_code)]
        impl $name {
            $(
                $(#[$fm])*
                #[inline(always)]
                #[must_use]
                #[allow(clippy::clone_on_copy)]
                pub fn $field(&self) -> $ty {
                    self.$field.clone()
                }
            )*
        }

        $(#[$m])*
        ///
        /// This build carries no fault switches: every accessor is the field type's
        /// `Default`, a constant (`cfg(not(any(test, feature = "controls")))`).
        #[cfg(not(any(test, feature = "controls")))]
        $vis struct $name {
            // Zero-sized, and not constructible outside this module: a production build
            // cannot turn a fault on.
            _off: (),
        }

        #[cfg(not(any(test, feature = "controls")))]
        #[allow(dead_code)]
        impl $name {
            $(
                $(#[$fm])*
                #[inline(always)]
                #[must_use]
                pub fn $field(&self) -> $ty {
                    <$ty as ::core::default::Default>::default()
                }
            )*
        }
    };
}
