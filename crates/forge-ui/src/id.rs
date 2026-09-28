//! Stable widget identity (Ch.21 §21.4).
//!
//! A [`WidgetId`] is `hash64(parent id, key)`. The hash is a fixed FNV-1a/`splitmix`
//! mix with no per-process seed, so an id is the same value **in every run**: it is the
//! AccessKit `NodeId`, where focus is restored, the selector UI tests use, and the key
//! for per-widget session state saved with the layout.

use std::sync::Arc;

/// Stable across frames and across runs: `hash64(parent WidgetId, Key)`.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct WidgetId(pub u64);

impl WidgetId {
    /// The id of a window's root widget.
    pub const ROOT: WidgetId = WidgetId(0x464F_5247_4555_4931); // "FORGEUI1"

    /// The id of the child of `self` keyed `key`.
    pub fn child(self, key: &Key) -> WidgetId {
        let mut h = Fnv::new();
        h.u64(self.0);
        key.hash_into(&mut h);
        WidgetId(splitmix(h.finish()))
    }

    /// The AccessKit node id (identical value, §21.10).
    pub fn to_accesskit(self) -> accesskit::NodeId {
        accesskit::NodeId(self.0)
    }
}

/// A child's key within its parent. A child in a dynamic collection **must** be keyed by
/// its model identity (`Key::Id`), so a reorder moves widgets instead of recreating them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Static(&'static str),
    Index(u32),
    Id(u64),
    Str(Arc<str>),
}

impl Key {
    fn hash_into(&self, h: &mut Fnv) {
        match self {
            Key::Static(s) => {
                h.byte(1);
                h.bytes(s.as_bytes());
            }
            // `Str` hashes like `Static` so the same name is the same id however it was built.
            Key::Str(s) => {
                h.byte(1);
                h.bytes(s.as_bytes());
            }
            Key::Index(i) => {
                h.byte(2);
                h.u64(u64::from(*i));
            }
            Key::Id(i) => {
                h.byte(3);
                h.u64(*i);
            }
        }
    }
}

impl From<&'static str> for Key {
    fn from(s: &'static str) -> Self {
        Key::Static(s)
    }
}

/// FNV-1a 64: fixed, seedless, platform-independent.
#[derive(Clone, Copy)]
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) const fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    pub(crate) fn byte(&mut self, b: u8) {
        self.0 ^= u64::from(b);
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
    }
    pub(crate) fn bytes(&mut self, bs: &[u8]) {
        for &b in bs {
            self.byte(b);
        }
        // Length-terminate so ("ab","c") and ("a","bc") differ.
        self.u64(bs.len() as u64);
    }
    pub(crate) fn u64(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.byte(b);
        }
    }
    pub(crate) fn u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.byte(b);
        }
    }
    pub(crate) fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    pub(crate) fn finish(self) -> u64 {
        self.0
    }
}

/// Stable 64-bit hash of a string (text cache keys).
pub(crate) fn hash_str(s: &str) -> u64 {
    let mut h = Fnv::new();
    h.bytes(s.as_bytes());
    h.finish()
}

/// Final avalanche so nearby inputs spread over the whole id space.
pub(crate) fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_across_runs() {
        // A committed constant: if the hash ever changes, every saved layout, a11y
        // bookmark and UI-test selector silently breaks. This pins it.
        let id = WidgetId::ROOT.child(&Key::Static("gallery"));
        assert_eq!(id, WidgetId::ROOT.child(&Key::Str("gallery".into())));
        assert_eq!(id.0, 0x5e4c_c7dd_ba31_787f, "got {:#x}", id.0);
    }

    #[test]
    fn keys_do_not_collide_across_kinds() {
        let p = WidgetId::ROOT;
        assert_ne!(p.child(&Key::Index(7)), p.child(&Key::Id(7)));
        assert_ne!(p.child(&Key::Index(1)), p.child(&Key::Index(2)));
    }
}
