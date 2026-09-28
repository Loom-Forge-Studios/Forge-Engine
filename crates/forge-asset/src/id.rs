//! [`AssetId`] — an asset's identity, independent of its path.
//!
//! An id is minted once, when a source is first imported, from the BLAKE3 of the path it was
//! imported at and its content at that moment; from then on it lives in the import sidecar
//! (`<source>.meta.ron`) and **travels with the file**. Renaming or moving the source keeps
//! the id, so every reference to it (a scene, a material, a prefab) survives the move — the
//! reason references name ids, never paths.
//!
//! * A sub-asset (a glTF's meshes, materials and embedded textures) is `parent.child(label)`:
//!   stable for as long as its parent's id and its label are.
//! * A **generated** asset (a terrain chunk, a scattered instance, a procedural texture) has a
//!   `SeedPath`, not a file (Ch.8 brief, Ch.33.2): [`AssetId::generated`] derives its id from
//!   the generator and the path, so it is the same on every machine and every run, and the
//!   asset system handles it exactly like a file-backed asset.

use std::fmt;
use std::str::FromStr;

use forge_seed::SeedPath;
use forge_store::{Blake3, StorePath};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::AssetError;

/// An asset's stable identity: 128 bits, displayed as 32 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub u128);

fn derive(domain: &[u8], parts: &[&[u8]]) -> AssetId {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    for p in parts {
        h.update(&(p.len() as u64).to_le_bytes());
        h.update(p);
    }
    let mut b = [0u8; 16];
    b.copy_from_slice(&h.finalize().as_bytes()[..16]);
    AssetId(u128::from_le_bytes(b))
}

impl AssetId {
    /// Mint the id of a source first imported at `path` with content address `content`.
    #[must_use]
    pub fn mint(path: &StorePath, content: &Blake3) -> Self {
        derive(
            b"forge-asset-id-v1/source",
            &[path.as_str().as_bytes(), &content.0],
        )
    }

    /// The id of the sub-asset `label` of this asset (`mesh:Hull`, `texture#2`).
    #[must_use]
    pub fn child(self, label: &str) -> Self {
        if label.is_empty() {
            return self;
        }
        derive(
            b"forge-asset-id-v1/sub",
            &[&self.0.to_le_bytes(), label.as_bytes()],
        )
    }

    /// The id of the asset `generator` produces at `seed`.
    #[must_use]
    pub fn generated(generator: &str, seed: &SeedPath) -> Self {
        derive(
            b"forge-asset-id-v1/generated",
            &[generator.as_bytes(), seed.to_string().as_bytes()],
        )
    }

    /// 32 lowercase hex digits.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("{:032x}", self.0)
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl fmt::Debug for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssetId({:012x}…)", self.0 >> 80)
    }
}

impl FromStr for AssetId {
    type Err = AssetError;
    fn from_str(s: &str) -> Result<Self, AssetError> {
        if s.len() != 32 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(AssetError::UnknownAsset(format!(
                "{s:?} (an asset id is 32 lowercase hex digits)"
            )));
        }
        u128::from_str_radix(s, 16)
            .map(Self)
            .map_err(|e| AssetError::UnknownAsset(format!("{s:?}: {e}")))
    }
}

impl Serialize for AssetId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for AssetId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> StorePath {
        StorePath::new(s).expect("path")
    }

    #[test]
    fn ids_round_trip_and_separate_their_inputs() {
        let a = AssetId::mint(&p("a/b.png"), &Blake3::of(b"x"));
        assert_eq!(a.to_hex().parse::<AssetId>().expect("parses"), a);
        assert_ne!(a, AssetId::mint(&p("a/c.png"), &Blake3::of(b"x")));
        assert_ne!(a, AssetId::mint(&p("a/b.png"), &Blake3::of(b"y")));
        assert_eq!(a.child(""), a);
        assert_ne!(a.child("mesh:A"), a.child("mesh:B"));
        let s = SeedPath::universe().child("chunk", 4);
        assert_eq!(AssetId::generated("g", &s), AssetId::generated("g", &s));
        assert_ne!(AssetId::generated("g", &s), AssetId::generated("h", &s));
        assert!("XYZ".parse::<AssetId>().is_err());
        assert!(
            "0123456789ABCDEF0123456789abcdef"
                .parse::<AssetId>()
                .is_err()
        );
    }
}
