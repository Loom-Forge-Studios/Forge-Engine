//! `forge-reflect` — reflection, annotation and the four outputs (Ch.6).
//!
//! **One annotation, four outputs.** `#[forge_api]` on a free fn, a struct with named fields
//! or an enum emits, from one parse of the item:
//!
//! 1. a **`bevy_reflect` registration** — the item's (or every signature type's) `TypeInfo`,
//!    registered in the [`ForgeRegistry`]'s `TypeRegistry` (inspector, serialiser);
//! 2. a **blueprint node descriptor** ([`NodeDesc`]: pins, types, units, docs, purity);
//! 3. a **JSON Schema fragment** (the API schema a protocol server serves);
//! 4. a **command variant** ([`CommandDesc`]) if it mutates project state (the bus, Ch.7).
//!
//! [`check_agreement`] proves the four still describe the same thing; registration runs it
//! on every item and `tests/liveness/test_four_outputs_agree.rs` runs it on every item in the
//! engine (with a positive control that drifts a descriptor and must fail).
//!
//! ```
//! use forge_reflect::{ForgeRegistry, Purity, forge_api};
//!
//! /// Kinetic energy of a moving mass.
//! #[forge_api(category = "Physics", returns(units = "J"))]
//! pub fn kinetic_energy(
//!     #[forge(units = "kg", min = 0.0)] mass: f64,
//!     #[forge(units = "m/s")] speed: f64,
//! ) -> f64 {
//!     0.5 * mass * speed * speed
//! }
//!
//! /// Clears every region's profile history (a mutating fn: it becomes a command).
//! #[forge_api(mutates, returns(min = 0, max = 1))]
//! pub fn reset(world: &mut forge_core::World, #[forge(range = 0..=1)] level: u8) -> u8 {
//!     level
//! }
//!
//! let mut reg = ForgeRegistry::new();
//! let item = reg.register::<kinetic_energy>().expect("agrees");
//! assert_eq!(item.node.purity, Purity::Pure);
//! assert_eq!(item.node.inputs[1].meta.units, Some("m/s"));
//! assert!(item.command.is_none());
//! assert_eq!(kinetic_energy(2.0, 3.0), 9.0); // the fn itself is untouched
//! let item = reg.register::<reset>().expect("agrees");
//! assert_eq!(item.command.as_ref().map(|c| c.variant.as_str()), Some("Reset"));
//! ```
//!
//! # Attributes (Ch.6 §6.2 — the inspector metadata WP-U6 generates editors from)
//!
//! On the item, `#[forge_api(...)]`: `name = "..."` (title), `category = "..."`, and for a fn
//! `pure` / `reads` / `mutates` (asserts the inferred purity), `destructive`, and
//! `returns(...)` (annotations for the return value). On a field or parameter,
//! `#[forge(...)]`: `name`, `category`, `units`, `min`, `max`, `step`, `range = a..=b`,
//! `read_only`, `hidden`, `widget = "..."`, `entity`, and `doc = "..."` (parameters only —
//! fields use their doc comment). See [`FieldMeta`].
//!
//! Compile-time refusals:
//!
//! ```compile_fail
//! /// A pin in furlongs.
//! #[forge_reflect::forge_api]
//! pub fn f(#[forge(units = "furlong")] x: f64) -> f64 { x }
//! ```
//!
//! ```compile_fail
//! #[forge_reflect::forge_api] // no doc comment: it is the node tooltip and the API description
//! pub fn undocumented(x: f64) -> f64 { x }
//! ```
//!
//! ```compile_fail
//! /// Claims to be pure but takes `&mut`.
//! #[forge_reflect::forge_api(pure)]
//! pub fn liar(w: &mut forge_core::World) {}
//! ```
//!
//! ```compile_fail
//! /// Empty range.
//! #[forge_reflect::forge_api]
//! pub fn g(#[forge(min = 2.0, max = 1.0)] x: f64) -> f64 { x }
//! ```

#![forbid(unsafe_code)]

// `::forge_reflect::...` paths in macro output resolve inside this crate too.
extern crate self as forge_reflect;

mod agree;
mod desc;
mod error;
mod registry;
mod types;
mod units;

pub use agree::check_agreement;
pub use desc::{
    ApiItem, CommandDesc, Connection, ContextParam, EnumSpec, FieldMeta, FnSpec, ForgeApi,
    ItemKind, NodeDesc, Pin, PinSpec, Purity, ReflectArg, ReflectDesc, StructSpec, VariantDesc,
    VariantSpec, connect, enum_schema, normalize_doc, struct_schema, upper_camel,
};
pub use error::ReflectError;
pub use forge_core::{CodedError, ErrorCode};
pub use forge_reflect_macros::forge_api;
pub use registry::{ForgeRegistry, InspectorDesc, InspectorField};
pub use types::{ForgeType, PinKind, SchemaDefs, TypeDesc};
pub use units::{BASE_SYMBOLS, Dims, Unit};

/// Re-exported so downstream crates reach the exact pinned version (Risk S6).
pub use bevy_reflect;

/// True in the `mutate-drift` build (the W2 positive-control mutant). The guard's control
/// uses it to avoid recursing into itself.
pub const MUTATE_DRIFT: bool = cfg!(feature = "mutate-drift");

/// Macro support. Not API: paths here change without notice.
#[doc(hidden)]
pub mod __private {
    pub use crate::desc::{
        ApiItem, ContextParam, EnumSpec, FieldMeta, FnSpec, ForgeApi, PinSpec, Purity, ReflectArg,
        StructSpec, VariantSpec, enum_schema, struct_schema,
    };
    pub use crate::types::{ForgeType, PinKind, SchemaDefs, TypeDesc};
    pub use bevy_reflect::{Reflect, TypePath, Typed};
    pub use serde_json::Value;

    /// Glob-imported by `#[forge_api]` next to each struct/enum it derives `Reflect` on:
    /// `bevy_reflect`'s derive names its crate as a bare `bevy_reflect::` path, and this puts
    /// that name in scope in crates that do not depend on `bevy_reflect` themselves. Glob
    /// imports of the same item never conflict, so any number of annotated items can share a
    /// module, and an explicit item or a direct dependency of the same name wins.
    pub mod reexport {
        pub use bevy_reflect;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reflect_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in ReflectError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-reflect |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(
                e.to_string().starts_with(code.as_str()),
                "Display leads with the code"
            );
        }
    }
}
