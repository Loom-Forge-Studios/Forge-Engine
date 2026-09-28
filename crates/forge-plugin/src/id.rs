//! [`PluginId`] and [`ItemId`] — the names on everything in the plugin system.

use std::fmt;
use std::sync::Arc;

use crate::PluginError;

/// A plugin's stable id: lowercase, dot-separated segments of `a-z 0-9 _ -`, at least two
/// (`com.example.rivers`). First-party plugins use the `forge.` prefix (`forge.panels`,
/// `forge.store`). Cheap to clone.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PluginId(Arc<str>);

impl PluginId {
    /// Longest accepted id, in bytes.
    pub const MAX_LEN: usize = 128;

    /// Validate and wrap `id`.
    pub fn new(id: &str) -> Result<Self, PluginError> {
        let segs: Vec<&str> = id.split('.').collect();
        let ok = id.len() <= Self::MAX_LEN
            && segs.len() >= 2
            && segs.iter().all(|s| {
                !s.is_empty()
                    && s.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                    })
            });
        if ok {
            Ok(Self(Arc::from(id)))
        } else {
            Err(PluginError::BadPluginId(id.to_string()))
        }
    }

    /// The id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True for the engine's own plugins (`forge.*`). They load through exactly the same
    /// path as any other plugin (I16); the flag only affects how the plugin manager lists
    /// them.
    #[must_use]
    pub fn is_first_party(&self) -> bool {
        self.0.starts_with("forge.")
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Check an item key: 1-128 bytes of `A-Z a-z 0-9 _ - . :`.
pub fn check_key(key: &str) -> Result<(), PluginError> {
    let ok = !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'));
    if ok {
        Ok(())
    } else {
        Err(PluginError::BadItemKey(key.to_string()))
    }
}

/// One item at one extension point: the point's id and the item's key
/// (`forge.editor.panel` / `forge.hierarchy`). Displayed as `point/key`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ItemId {
    point: &'static str,
    key: Arc<str>,
}

impl ItemId {
    /// The item `key` of extension point `P`.
    pub fn of<P: crate::ExtensionPoint>(key: &str) -> Result<Self, PluginError> {
        check_key(key)?;
        Ok(Self {
            point: P::ID,
            key: Arc::from(key),
        })
    }

    pub(crate) fn from_parts(point: &'static str, key: Arc<str>) -> Self {
        Self { point, key }
    }

    /// The extension point's id.
    #[must_use]
    pub fn point(&self) -> &'static str {
        self.point
    }

    /// The item's key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.point, self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_ids_are_validated() {
        for ok in ["com.example.rivers", "forge.panels", "a.b", "x-1.y_2"] {
            assert!(PluginId::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "rivers",
            "Com.example",
            "a..b",
            ".a.b",
            "a.b.",
            "a b.c",
            "a.b/c",
        ] {
            assert_eq!(
                PluginId::new(bad).map_err(|e| e.code().as_str()),
                Err("PLUGIN-0001"),
                "{bad:?}"
            );
        }
        assert!(PluginId::new("forge.store").map(|p| p.is_first_party()) == Ok(true));
        assert!(PluginId::new("forgery.x").map(|p| p.is_first_party()) == Ok(false));
    }

    #[test]
    fn item_keys_are_validated() {
        assert!(check_key("forge.river").is_ok());
        assert!(check_key("river_v2").is_ok());
        assert!(check_key("").is_err());
        assert!(check_key("a b").is_err());
        assert!(check_key(&"x".repeat(129)).is_err());
    }
}
