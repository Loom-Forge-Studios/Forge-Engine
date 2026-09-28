//! Seed provenance (Ch.4.3): `SeedPath`, and the debug-build recorder behind `Seed::path`.

use core::fmt;
use core::str::FromStr;

use crate::Seed;

/// How a seed was derived from its root:
/// `universe/world:3/zone:118/tree:2/region:14883`.
///
/// `Display` and `FromStr` round-trip, so a path pasted from a bug report replays to the
/// same seed with [`SeedPath::resolve`]. Tags containing `/`, `:` or `%` are
/// percent-escaped in the text form.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct SeedPath {
    segments: Vec<(String, u64)>,
}

/// The root segment of every path.
const ROOT: &str = "universe";

impl SeedPath {
    /// The root itself (its text is `ROOT`, the name every path starts with).
    pub fn universe() -> Self {
        Self::default()
    }

    /// This path extended by one `child(tag, index)` step.
    pub fn child(mut self, tag: &str, index: u64) -> Self {
        self.segments.push((tag.to_string(), index));
        self
    }

    /// `(tag, index)` per derivation step, root first.
    pub fn segments(&self) -> impl Iterator<Item = (&str, u64)> {
        self.segments.iter().map(|(t, i)| (t.as_str(), *i))
    }

    /// Number of derivation steps below the root.
    pub fn depth(&self) -> usize {
        self.segments.len()
    }

    /// Replay the path from `root`: `O(depth)`.
    pub fn resolve(&self, root: Seed) -> Seed {
        self.segments
            .iter()
            .fold(root, |s, (tag, index)| s.child_dyn(tag, *index))
    }
}

fn escape(tag: &str, out: &mut fmt::Formatter<'_>) -> fmt::Result {
    for c in tag.chars() {
        match c {
            '%' => out.write_str("%25")?,
            '/' => out.write_str("%2F")?,
            ':' => out.write_str("%3A")?,
            c => write!(out, "{c}")?,
        }
    }
    Ok(())
}

fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let code = rest.get(i + 1..i + 3)?;
        out.push(match code {
            "25" => '%',
            "2F" => '/',
            "3A" => ':',
            _ => return None,
        });
        rest = &rest[i + 3..];
    }
    out.push_str(rest);
    Some(out)
}

impl fmt::Display for SeedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(ROOT)?;
        for (tag, index) in &self.segments {
            f.write_str("/")?;
            escape(tag, f)?;
            write!(f, ":{index}")?;
        }
        Ok(())
    }
}

/// Why a string is not a [`SeedPath`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeedPathError {
    /// `SEED-0001`: the text does not start with the root's name.
    MissingRoot,
    /// `SEED-0002`: a segment is not `tag:index` with a `u64` index and a valid escape.
    BadSegment(String),
}

impl SeedPathError {
    /// The stable error code (`docs/error-codes.md`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingRoot => "SEED-0001",
            Self::BadSegment(_) => "SEED-0002",
        }
    }
}

impl fmt::Display for SeedPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRoot => write!(f, "{}: a seed path starts with `{ROOT}`", self.code()),
            Self::BadSegment(s) => write!(
                f,
                "{}: `{s}` is not a `tag:index` segment (index a u64; escapes %25 %2F %3A)",
                self.code()
            ),
        }
    }
}

impl std::error::Error for SeedPathError {}

impl FromStr for SeedPath {
    type Err = SeedPathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('/');
        if parts.next() != Some(ROOT) {
            return Err(SeedPathError::MissingRoot);
        }
        let mut path = SeedPath::universe();
        for seg in parts {
            let bad = || SeedPathError::BadSegment(seg.to_string());
            let (tag, index) = seg.rsplit_once(':').ok_or_else(bad)?;
            let index: u64 = index.parse().map_err(|_| bad())?;
            let tag = unescape(tag).ok_or_else(bad)?;
            if tag.is_empty() {
                return Err(bad());
            }
            path.segments.push((tag, index));
        }
        Ok(path)
    }
}

// ---- the debug-build recorder -------------------------------------------------------------

#[cfg(debug_assertions)]
pub(crate) use recorder::{PathHandle, lookup, record_child, record_root};

/// Records every distinct derivation step once (interned), so a `Seed` stays `Copy` and
/// 16 bytes in debug builds. Values never read the recorder; only `Seed::path` does.
/// Capped at [`recorder::CAP`] distinct steps: past it, new seeds report no path rather
/// than growing without bound in a long debug session.
#[cfg(debug_assertions)]
mod recorder {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    use super::SeedPath;

    /// Distinct derivation steps recorded before the recorder stops (about 100 MB).
    pub(super) const CAP: usize = 1 << 20;
    const UNRECORDED: u32 = u32::MAX;
    const ROOT_NODE: u32 = u32::MAX - 1;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct PathHandle(u32);

    #[derive(Default)]
    struct Recorder {
        /// (parent handle, tag, index)
        nodes: Vec<(u32, Box<str>, u64)>,
        /// (parent, index, fnv1a(tag)) → node; a tag-hash collision is simply not interned.
        index: HashMap<(u32, u64, u64), u32>,
    }

    fn recorder() -> &'static Mutex<Recorder> {
        static R: OnceLock<Mutex<Recorder>> = OnceLock::new();
        R.get_or_init(Mutex::default)
    }

    pub(crate) fn record_root(_value: u64) -> PathHandle {
        PathHandle(ROOT_NODE)
    }

    pub(crate) fn record_child(parent: PathHandle, tag: &str, index: u64) -> PathHandle {
        if parent.0 == UNRECORDED {
            return parent;
        }
        let key = (parent.0, index, crate::fnv1a(tag));
        let mut r = recorder().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&n) = r.index.get(&key)
            && *r.nodes[n as usize].1 == *tag
        {
            return PathHandle(n);
        }
        if r.nodes.len() >= CAP {
            return PathHandle(UNRECORDED);
        }
        let n = r.nodes.len() as u32;
        r.nodes.push((parent.0, tag.into(), index));
        r.index.entry(key).or_insert(n);
        PathHandle(n)
    }

    pub(crate) fn lookup(h: PathHandle) -> Option<SeedPath> {
        if h.0 == UNRECORDED {
            return None;
        }
        let r = recorder().lock().unwrap_or_else(|e| e.into_inner());
        let mut rev = Vec::new();
        let mut cur = h.0;
        while cur != ROOT_NODE {
            let (parent, tag, index) = r.nodes.get(cur as usize)?;
            rev.push((tag.to_string(), *index));
            cur = *parent;
        }
        rev.reverse();
        Some(SeedPath { segments: rev })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_and_resolves() {
        let p: SeedPath = "universe/world:3/zone:118/tree:2/region:14883"
            .parse()
            .unwrap();
        assert_eq!(p.depth(), 4);
        assert_eq!(
            p.to_string(),
            "universe/world:3/zone:118/tree:2/region:14883"
        );
        let u = Seed::root(0xF0F0);
        let direct = u
            .child("world", 3)
            .child("zone", 118)
            .child("tree", 2)
            .child("region", 14883);
        assert_eq!(p.resolve(u), direct);
    }

    #[test]
    fn awkward_tags_are_escaped() {
        let p = SeedPath::universe().child("a/b:c%d", u64::MAX);
        let s = p.to_string();
        assert_eq!(s, format!("universe/a%2Fb%3Ac%25d:{}", u64::MAX));
        assert_eq!(s.parse::<SeedPath>().unwrap(), p);
    }

    #[test]
    fn malformed_paths_are_rejected_with_codes() {
        assert_eq!(
            "world:3".parse::<SeedPath>(),
            Err(SeedPathError::MissingRoot)
        );
        for bad in [
            "universe/world",
            "universe/world:x",
            "universe/:3",
            "universe/ga%zzlaxy:3",
            "universe/world:-1",
        ] {
            let e = bad.parse::<SeedPath>().unwrap_err();
            assert_eq!(e.code(), "SEED-0002", "{bad}: {e}");
        }
        assert_eq!("universe".parse::<SeedPath>(), Ok(SeedPath::universe()));
    }

    #[test]
    fn seeds_record_their_path_in_debug_builds_only() {
        let s = Seed::root(99).child("world", 3).child("zone", 118);
        if cfg!(debug_assertions) {
            let p = s.path().unwrap();
            assert_eq!(p.to_string(), "universe/world:3/zone:118");
            assert_eq!(p.resolve(Seed::root(99)), s);
            assert_eq!(Seed::root(5).path(), Some(SeedPath::universe()));
        } else {
            assert_eq!(s.path(), None);
        }
    }

    #[test]
    fn the_recorder_interns_repeated_derivations() {
        let a = Seed::root(1).child("interned", 77);
        let b = Seed::root(2).child("interned", 77);
        // Same path from different roots: same recorded path, different values.
        assert_eq!(a.path(), b.path());
        assert_ne!(a, b);
    }
}
