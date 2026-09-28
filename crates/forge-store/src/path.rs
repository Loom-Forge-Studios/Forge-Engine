//! [`StorePath`] and [`Blake3`] — how a store names files and content.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use crate::StoreError;

/// A project-relative path that means the same file on every backend and every platform:
/// `/`-separated, no empty, `.` or `..` segment, none of `\ : * ? " < > |` or control
/// characters, no segment ending in a space or dot, no Windows device name (`CON`, `nul.txt`,
/// `COM1`, ...), at most 1024 bytes (255 per segment), and not under the store's own `.forge/`.
/// A path that works on Linux but breaks on Windows is refused on both (M0-17's bug class).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct StorePath(Arc<str>);

const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

impl StorePath {
    /// Validate `path`.
    pub fn new(path: &str) -> Result<Self, StoreError> {
        let bad = |why: &'static str| StoreError::BadPath {
            path: path.to_string(),
            why,
        };
        if path.is_empty() {
            return Err(bad("empty"));
        }
        if path.len() > 1024 {
            return Err(bad("longer than 1024 bytes"));
        }
        for (i, seg) in path.split('/').enumerate() {
            if seg.is_empty() {
                return Err(bad("empty segment (leading, trailing or doubled `/`)"));
            }
            if seg == "." || seg == ".." {
                return Err(bad("`.` and `..` segments are not allowed"));
            }
            if seg.len() > 255 {
                return Err(bad("a segment is longer than 255 bytes"));
            }
            if seg.chars().any(|c| {
                c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            }) {
                return Err(bad(
                    "contains one of \\ : * ? \" < > | or a control character",
                ));
            }
            if seg.ends_with(' ') || seg.ends_with('.') {
                return Err(bad("a segment ends in a space or a dot (Windows drops it)"));
            }
            let stem = seg.split('.').next().unwrap_or(seg).to_ascii_lowercase();
            if RESERVED.contains(&stem.as_str()) {
                return Err(bad(
                    "a segment is a Windows device name (CON, NUL, COM1, ...)",
                ));
            }
            if i == 0 && seg.eq_ignore_ascii_case(".forge") {
                return Err(bad("`.forge/` belongs to the store itself"));
            }
        }
        Ok(Self(Arc::from(path)))
    }

    /// The path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment (`ship.gltf` of `models/ship.gltf`).
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Its segments.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// The case-folded form two paths collide on.
    #[must_use]
    pub fn folded(&self) -> String {
        self.0.to_lowercase()
    }
}

impl fmt::Display for StorePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A BLAKE3 content address (Ch.33.2). Displayed as 64 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Blake3(pub [u8; 32]);

impl Blake3 {
    /// The address of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// 64 lowercase hex digits.
    #[must_use]
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push(char::from(HEX[usize::from(b >> 4)]));
            s.push(char::from(HEX[usize::from(b & 15)]));
        }
        s
    }
}

impl fmt::Display for Blake3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Blake3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Blake3({})", &self.to_hex()[..12])
    }
}

impl FromStr for Blake3 {
    type Err = StoreError;
    fn from_str(s: &str) -> Result<Self, StoreError> {
        let bad = || StoreError::BadRevId(s.to_string());
        if s.len() != 64 {
            return Err(bad());
        }
        let digit = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let mut out = [0u8; 32];
        for (i, pair) in s.as_bytes().chunks(2).enumerate() {
            let (Some(hi), Some(lo)) = (digit(pair[0]), digit(pair[1])) else {
                return Err(bad());
            };
            out[i] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_paths_only() {
        for ok in [
            "scene.ron",
            "levels/a/b.ron",
            "assets/tex/rock_01.png",
            ".gitignore",
            "con_fig.ron",
            "a.b.c",
        ] {
            assert!(StorePath::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/abs",
            "a//b",
            "a/",
            "./a",
            "a/../b",
            "a\\b",
            "c:/x",
            "what?.ron",
            "tab\there",
            "trailing ",
            "dot.",
            "CON",
            "nul.txt",
            "dir/com1.png",
            ".forge/HEAD",
            ".FORGE/x",
        ] {
            assert_eq!(
                StorePath::new(bad).map_err(|e| e.code().as_str()),
                Err("STORE-0001"),
                "{bad:?}"
            );
        }
        assert!(StorePath::new(&"a".repeat(256)).is_err());
        assert!(StorePath::new(&format!("{}/x", "a/".repeat(600))).is_err());
    }

    #[test]
    fn blake3_hex_round_trips_and_matches_the_reference_vector() {
        // BLAKE3 of the empty input (the published test vector).
        let empty = Blake3::of(b"");
        assert_eq!(
            empty.to_hex(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert_eq!(empty.to_hex().parse::<Blake3>(), Ok(empty));
        assert!("xyz".parse::<Blake3>().is_err());
        assert!(
            "A".repeat(64).parse::<Blake3>().is_err(),
            "uppercase is not canonical"
        );
    }
}
