//! `forge-seed` — seed algebra (Ch.4).
//!
//! A root seed is one `u64`. Everything else **derives** (invariant I3):
//! `root.child("world", 3).child("zone", 118).child("tree", 2)` is computable in
//! `O(depth)` from nothing but the root seed and the path. No seed is ever accumulated,
//! stored as mutable state, or advanced like a stream: [`Seed`] is `Copy`, has no `&mut`
//! method, and has no notion of "next". The guard is
//! `tests/determinism/test_seed_algebra.rs`: every registered generator ([`BASIS`], and each
//! later crate's list) gives identical output whatever order its inputs are evaluated in.
//!
//! In debug builds every seed also carries its derivation path ([`Seed::path`] →
//! `universe/world:3/zone:118/tree:2/region:14883`, Ch.4.3), which makes "why is this
//! mountain here" answerable and a bug report reproducible from a string
//! ([`SeedPath::resolve`]). Release builds carry the `u64` and nothing else.

#![forbid(unsafe_code)]

mod path;
mod registry;

pub use path::{SeedPath, SeedPathError};
pub use registry::{BASIS, Generator, check_order_independence, shuffled};

use core::fmt;
use core::hash::{Hash, Hasher};

/// SplitMix64's output function applied to `x + 0x9E37_79B9_7F4A_7C15` — i.e. the value a
/// SplitMix64 generator whose state is `x` returns next (Steele, Lea & Flood 2014). A
/// bijection on `u64` with full avalanche.
#[inline]
pub const fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 64-bit FNV-1a of the UTF-8 bytes of `s` (offset basis `0xcbf29ce484222325`, prime
/// `0x100000001b3`).
#[inline]
pub const fn fnv1a(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    let mut i = 0;
    while i < b.len() {
        h ^= b[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
        i += 1;
    }
    h
}

/// The Ch.4.1 derivation, exactly:
/// `splitmix64(parent ^ fnv1a(tag).rotate_left(17) ^ splitmix64(index))`.
#[inline]
pub(crate) const fn derive(parent: u64, tag: &str, index: u64) -> u64 {
    splitmix64(parent ^ fnv1a(tag).rotate_left(17) ^ splitmix64(index))
}

/// A seed: a `u64` that is only ever *derived*, never advanced (I3).
///
/// Equality, ordering and hashing use the value alone. In debug builds the seed also holds
/// a handle to its recorded derivation path (see [`Seed::path`]); the handle never
/// influences a value.
#[derive(Clone, Copy)]
pub struct Seed {
    value: u64,
    #[cfg(debug_assertions)]
    path: path::PathHandle,
}

impl Seed {
    /// The root: the one `u64` everything else derives from. Its path is the root path,
    /// `SeedPath::universe()`.
    #[inline]
    pub fn root(value: u64) -> Seed {
        Seed {
            value,
            #[cfg(debug_assertions)]
            path: path::record_root(value),
        }
    }

    /// Deterministic, associative-free, collision-resistant descent (Ch.4.1).
    /// `tag` is a compile-time string so the derivation path is greppable.
    #[inline]
    pub fn child(self, tag: &'static str, index: u64) -> Seed {
        self.child_dyn(tag, index)
    }

    /// [`Seed::child`] for a tag known only at run time — used to replay a parsed
    /// [`SeedPath`]. Engine code derives with `child` and a literal tag.
    #[inline]
    pub(crate) fn child_dyn(self, tag: &str, index: u64) -> Seed {
        Seed {
            value: derive(self.value, tag, index),
            #[cfg(debug_assertions)]
            path: path::record_child(self.path, tag, index),
        }
    }

    /// The raw value, for handing to `forge_num`'s noise basis (which takes a `u64` because
    /// it sits below this crate, ADR 0003). Read it; never store it as state.
    #[inline]
    pub const fn raw(self) -> u64 {
        self.value
    }

    /// This seed's derivation path (Ch.4.3) — recorded in debug builds only; `None` in
    /// release builds, or if the debug recorder hit its size cap.
    #[inline]
    pub fn path(self) -> Option<SeedPath> {
        #[cfg(debug_assertions)]
        {
            path::lookup(self.path)
        }
        #[cfg(not(debug_assertions))]
        {
            None
        }
    }
}

impl PartialEq for Seed {
    fn eq(&self, o: &Self) -> bool {
        self.value == o.value
    }
}

impl Eq for Seed {}

impl Hash for Seed {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.value.hash(h);
    }
}

impl PartialOrd for Seed {
    fn partial_cmp(&self, o: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Seed {
    fn cmp(&self, o: &Self) -> core::cmp::Ordering {
        self.value.cmp(&o.value)
    }
}

impl fmt::Debug for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Seed({:#018x}", self.value)?;
        if let Some(p) = self.path() {
            write!(f, " @ {p}")?;
        }
        write!(f, ")")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_is_the_4_1_formula() {
        let s = Seed::root(0x2A);
        let c = s.child("world", 3);
        let want = splitmix64(0x2A ^ fnv1a("world").rotate_left(17) ^ splitmix64(3));
        assert_eq!(c.raw(), want);
    }

    #[test]
    fn equality_ignores_the_recorded_path() {
        // Same value reached two ways (the root seed of one equals a child of the other).
        let a = Seed::root(7).child("x", 1);
        let b = Seed::root(a.raw());
        assert_eq!(a, b);
    }

    #[test]
    fn debug_output_shows_the_path_in_debug_builds() {
        let s = Seed::root(1).child("world", 3);
        let d = format!("{s:?}");
        assert!(d.starts_with("Seed(0x"), "{d}");
        if cfg!(debug_assertions) {
            let want = format!("@ {})", SeedPath::universe().child("world", 3));
            assert!(d.ends_with(&want), "{d}");
        }
    }
}
