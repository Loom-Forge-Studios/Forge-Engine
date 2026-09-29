//! Finding premium names in text: the one matcher the boundary check, the doc scrub and the
//! export's leak scan all use, so the three can never disagree about what a hit is.
//!
//! * A **hard identifier** matches case-sensitively as a whole word: the character before
//!   it and the character after it are not word characters (letters, digits, `_`), where
//!   the identifier itself starts or ends with one. A trailing plural `s` still matches
//!   (`FooTrees` names `FooTree`).
//! * A **doc term** matches case-insensitively at the start of a word, with any ending
//!   (`hush` matches `Hushed`, `hushes`, `has_no_hush` and the CamelCase hump in
//!   `SeedHush`; not `unhush`).
//! * A **base phrase** (`premium.ron` `base_phrases`, WP-48) is a base feature whose name
//!   holds a term word (`volumetric fog`, `erosion brush`, `nav agent`). It is blanked out
//!   before the terms match, wherever it starts a word, in any case and with any one
//!   separator between its words (a space, `_`, `-`, or none: `VolumetricFog`,
//!   `volumetric_fog`, `Volumetric fog`), so the term still catches every other use of its
//!   word, the same sentence included. The hard identifiers see through it.

use std::borrow::Cow;
use std::collections::BTreeSet;

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[derive(Debug, Clone, Default)]
pub struct Matcher {
    hard: Vec<String>,
    /// The hard identifiers of the form `PREFIX<number>` (`M7-36`, `ERR-0006`, `I9`):
    /// `(prefix, number, id)`. A range citation spanning one names it (`M7-35..M7-39`).
    numbered: Vec<(String, u32, String)>,
    terms: Vec<String>,
    /// Exact, case-sensitive spellings blanked out of a text before the terms are matched
    /// (never before the hard identifiers): a base name that holds a term word but is not
    /// the premium thing (the seed algebra's root, `SeedPath::universe()`, ADR 0062).
    exempt: Vec<String>,
    /// Base phrases, as lowercase ASCII words (see the module docs).
    base: Vec<Vec<String>>,
}

/// `(prefix, number)` of a numbered id: capitals and digits ending in `-`, then digits
/// (`M7-36` gives `("M7-", 36)`), or capitals, then digits (`I9` gives `("I", 9)`).
pub fn split_id(id: &str) -> Option<(&str, u32)> {
    let digits = id.bytes().rev().take_while(u8::is_ascii_digit).count();
    let (prefix, n) = id.split_at(id.len() - digits);
    // `M2-`, `FRAMES-`, `I`: capitals and digits ending in `-`, or capitals only.
    let ok = digits > 0
        && prefix.starts_with(|c: char| c.is_ascii_uppercase())
        && ((prefix.ends_with('-')
            && prefix[..prefix.len() - 1]
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
            || prefix.bytes().all(|b| b.is_ascii_uppercase()));
    if !ok {
        return None;
    }
    Some((prefix, n.parse().ok()?))
}

/// Every id range in `text` (`M7-35..M7-39`, `ERR-0001..0006`): `(start, end, prefix,
/// low, high)`, byte offsets of the whole range.
pub fn id_ranges(text: &str) -> Vec<(usize, usize, String, u32, u32)> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    for (dots, _) in text.match_indices("..") {
        // Left: PREFIX then digits, ending right before the dots.
        let mut i = dots;
        while i > 0 && b[i - 1].is_ascii_digit() {
            i -= 1;
        }
        let lo_start = i;
        while i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'-') {
            i -= 1;
        }
        let Some((prefix, lo)) = split_id(&text[i..dots]) else {
            continue;
        };
        if lo_start == dots || (i > 0 && (b[i - 1] == b'_' || b[i - 1] == b'.')) {
            continue;
        }
        // Right: optionally the same prefix, then digits.
        let mut j = dots + 2;
        if text[j..].starts_with(prefix) {
            j += prefix.len();
        }
        let hi_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == hi_start || (j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_')) {
            continue;
        }
        let Ok(hi) = text[hi_start..j].parse::<u32>() else {
            continue;
        };
        if lo <= hi {
            out.push((i, j, prefix.to_string(), lo, hi));
        }
    }
    out
}

impl Matcher {
    pub fn new(hard: Vec<String>, terms: Vec<String>) -> Self {
        let mut hard: Vec<String> = hard.into_iter().filter(|h| !h.is_empty()).collect();
        hard.sort();
        hard.dedup();
        let numbered = hard
            .iter()
            .filter_map(|h| split_id(h).map(|(p, n)| (p.to_string(), n, h.clone())))
            .collect();
        let mut terms: Vec<String> = terms
            .into_iter()
            .filter(|t| !t.is_empty())
            .map(|t| t.to_lowercase())
            .collect();
        terms.sort();
        terms.dedup();
        Self {
            hard,
            numbered,
            terms,
            exempt: Vec::new(),
            base: Vec::new(),
        }
    }

    /// This matcher with the `exempt` spellings blanked out of a text before its terms are
    /// matched (the hard identifiers still see them).
    #[must_use]
    pub fn exempting(mut self, exempt: Vec<String>) -> Self {
        self.exempt = exempt.into_iter().filter(|e| !e.is_empty()).collect();
        self
    }

    /// This matcher with the base phrases blanked out of a text before its terms are matched
    /// (the module docs; the hard identifiers still see them). Each phrase is split into
    /// words at whitespace and lowercased; a phrase that is not ASCII is ignored (the
    /// manifest's validation refuses one).
    #[must_use]
    pub fn base_phrases(mut self, phrases: &[String]) -> Self {
        self.base = phrases
            .iter()
            .filter(|p| p.is_ascii())
            .map(|p| {
                p.split_whitespace()
                    .map(str::to_ascii_lowercase)
                    .collect::<Vec<_>>()
            })
            .filter(|w| !w.is_empty())
            .collect();
        self
    }

    /// `text` with every exempt spelling and base phrase replaced by as many spaces, so what
    /// follows it still starts a word.
    fn masked<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut out = Cow::Borrowed(text);
        for e in &self.exempt {
            if out.contains(e.as_str()) {
                out = Cow::Owned(out.replace(e.as_str(), &" ".repeat(e.len())));
            }
        }
        for words in &self.base {
            let spans = phrase_spans(&out, words);
            if !spans.is_empty() {
                let mut s = out.into_owned();
                for (a, b) in spans {
                    // A span is ASCII (it matched ASCII words), so byte offsets are char
                    // boundaries and the replacement keeps its length.
                    s.replace_range(a..b, &" ".repeat(b - a));
                }
                out = Cow::Owned(s);
            }
        }
        out
    }

    /// A matcher that sees only the hard identifiers (for non-prose files).
    pub fn hard_only(&self) -> Self {
        Self {
            hard: self.hard.clone(),
            numbered: self.numbered.clone(),
            terms: Vec::new(),
            exempt: Vec::new(),
            base: Vec::new(),
        }
    }

    /// The numbered identifiers a range citation in `text` spans.
    fn range_hits(&self, text: &str) -> BTreeSet<String> {
        if self.numbered.is_empty() || !text.contains("..") {
            return BTreeSet::new();
        }
        let mut out = BTreeSet::new();
        for (_, _, prefix, lo, hi) in id_ranges(text) {
            for (p, n, id) in &self.numbered {
                if *p == prefix && (lo..=hi).contains(n) {
                    out.insert(id.clone());
                }
            }
        }
        out
    }

    /// Every hard identifier `text` names (a range citation spanning one names it).
    pub fn hard_hits(&self, text: &str) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = self
            .hard
            .iter()
            .filter(|id| names(text, id))
            .cloned()
            .collect();
        out.extend(self.range_hits(text));
        out
    }

    /// The first hard identifier or doc term `text` names.
    pub fn first_hit(&self, text: &str) -> Option<String> {
        if let Some(h) = self.hard.iter().find(|id| names(text, id)) {
            return Some(h.clone());
        }
        if let Some(h) = self.range_hits(text).into_iter().next() {
            return Some(h);
        }
        if self.terms.is_empty() {
            return None;
        }
        let (lower, starts) = lower_with_starts(&self.masked(text));
        self.terms
            .iter()
            .find(|t| starts_word(&lower, &starts, t))
            .cloned()
    }

    pub fn hit(&self, text: &str) -> bool {
        self.first_hit(text).is_some()
    }

    /// Every term (doc or code term) `text` names at the start of a word.
    pub fn term_hits(&self, text: &str) -> BTreeSet<String> {
        if self.terms.is_empty() {
            return BTreeSet::new();
        }
        let (lower, starts) = lower_with_starts(&self.masked(text));
        self.terms
            .iter()
            .filter(|t| starts_word(&lower, &starts, t))
            .cloned()
            .collect()
    }

    /// Every hard identifier and every term `text` names.
    pub fn all_hits(&self, text: &str) -> BTreeSet<String> {
        let mut out = self.hard_hits(text);
        out.extend(self.term_hits(text));
        out
    }
}

/// Does `text` name the hard identifier `id` (whole word, case-sensitive)?
pub fn names(text: &str, id: &str) -> bool {
    let (Some(first), Some(last)) = (id.chars().next(), id.chars().next_back()) else {
        return false;
    };
    text.match_indices(id).any(|(i, _)| {
        let before_ok = !is_word(first) || !text[..i].chars().next_back().is_some_and(is_word);
        if !before_ok {
            return false;
        }
        if !is_word(last) {
            return true;
        }
        let mut after = text[i + id.len()..].chars();
        match after.next() {
            None => true,
            Some(c) if !is_word(c) => true,
            Some('s') => !after.next().is_some_and(is_word),
            Some(_) => false,
        }
    })
}

/// `text` lowercased, with a flag per byte of the result: does a word start there? A word
/// starts after a character that is not alphanumeric, and at a CamelCase hump (an upper-case
/// letter after a lower-case letter or digit: `SeedHush` holds the word `Hush`).
fn lower_with_starts(text: &str) -> (String, Vec<bool>) {
    let mut lower = String::with_capacity(text.len());
    let mut starts = Vec::with_capacity(text.len());
    let mut prev: Option<char> = None;
    for c in text.chars() {
        let start = match prev {
            None => true,
            Some(p) if !p.is_alphanumeric() => true,
            Some(p) => c.is_uppercase() && (p.is_lowercase() || p.is_ascii_digit()),
        };
        let before = lower.len();
        lower.extend(c.to_lowercase());
        starts.push(start);
        starts.extend(std::iter::repeat_n(false, lower.len() - before - 1));
        prev = Some(c);
    }
    (lower, starts)
}

/// Byte spans of `text` holding the phrase `words` (lowercase ASCII): starting a word (after a
/// character that is not alphanumeric, or at a CamelCase hump, as [`lower_with_starts`]), any
/// case, each word after the first optionally preceded by one of ` `, `_`, `-`.
pub fn phrase_spans(text: &str, words: &[String]) -> Vec<(usize, usize)> {
    let Some((first, rest)) = words.split_first() else {
        return Vec::new();
    };
    let b = text.as_bytes();
    let at = |j: usize, w: &str| {
        b.get(j..j + w.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(w.as_bytes()))
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        // A match is ASCII, so `i` is a char boundary whenever `at` holds.
        let starts = at(i, first) && {
            let prev = text[..i].chars().next_back();
            match prev {
                None => true,
                Some(p) if !p.is_alphanumeric() => true,
                Some(p) => b[i].is_ascii_uppercase() && (p.is_lowercase() || p.is_ascii_digit()),
            }
        };
        if starts {
            let mut j = i + first.len();
            let mut whole = true;
            for w in rest {
                if matches!(b.get(j), Some(b' ' | b'_' | b'-')) && !at(j, w) {
                    j += 1;
                }
                if !at(j, w) {
                    whole = false;
                    break;
                }
                j += w.len();
            }
            if whole {
                out.push((i, j));
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Does `lower` contain `term` where a word starts (see [`lower_with_starts`])?
fn starts_word(lower: &str, starts: &[bool], term: &str) -> bool {
    lower
        .match_indices(term)
        .any(|(i, _)| starts.get(i).copied().unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_identifiers_match_whole_words_only() {
        assert!(names("use forge_secret::X;", "forge_secret"));
        assert!(names("dep forge-secret = {", "forge-secret"));
        assert!(names("crates/forge-secret/src", "forge-secret"));
        assert!(!names("forge-secretive", "forge-secret"));
        assert!(!names("my_forge_secret", "forge_secret"));
        assert!(names("two SecretTrees here", "SecretTree"));
        assert!(!names("SecretTreeView", "SecretTree"));
        assert!(names("the SECRET server", "SECRET"));
        assert!(!names("the secret server", "SECRET"));
        assert!(names("x presets/hidden/a.ron", "presets/hidden"));
        assert!(names("\"forge.preset.hidden\"", "forge.preset.hidden"));
    }

    #[test]
    fn doc_terms_match_word_starts_in_any_case() {
        let m = Matcher::new(vec![], vec!["hush".into()]);
        assert!(m.hit("Hushed voices"));
        assert!(m.hit("the hushes"));
        assert!(!m.hit("unhush"));
        assert!(m.hit("a (hush) b"));
        assert!(m.hit("SeedHushed") && m.hit("has_no_hush_cost") && m.hit("x2Hush"));
        assert!(!m.hit("Seedhushed") && !m.hit("SEEDHUSH"));
        assert_eq!(m.hard_only().first_hit("hush"), None);
    }

    #[test]
    fn a_range_spanning_a_numbered_identifier_names_it() {
        let m = Matcher::new(
            vec!["M7-36".into(), "ERR-0006".into(), "I94".into()],
            vec![],
        );
        assert!(m.hit("see M7-35..M7-39 for these"));
        assert!(m.hit("(ERR-0001..0006)") && m.hit("I90..I99"));
        assert_eq!(m.first_hit("M7-30..M7-39"), Some("M7-36".into()));
        assert!(!m.hit("M7-37..M7-39") && !m.hit("ERR-0007..0009") && !m.hit("M3-35..M3-39"));
        assert!(!m.hit("M7-36x..M2-40") && !m.hit("range 1..40") && !m.hit("I95..I99"));
        assert_eq!(split_id("C-x-s8"), None);
        assert_eq!(split_id("ERR-0006"), Some(("ERR-", 6)));
        assert_eq!(
            id_ranges("x M2-1..4 y")
                .into_iter()
                .map(|r| (r.2, r.3, r.4))
                .collect::<Vec<_>>(),
            vec![("M2-".to_string(), 1, 4)]
        );
    }

    #[test]
    fn exempt_spellings_hide_only_themselves_from_the_terms() {
        let m = Matcher::new(vec!["hush-crate".into()], vec!["hush".into()])
            .exempting(vec!["Seed::hush()".into(), "`hush/".into()]);
        assert!(!m.hit("let s = Seed::hush();") && !m.hit("the path `hush/a:1`"));
        // Any other spelling on the same line is still a term.
        assert!(m.hit("Seed::hush() opens the hush navigator"));
        assert!(m.hit("Seed::hushed()") && m.hit("\"hush/a:1\"") && m.hit("Seed::hush"));
        // The hard identifiers see through an exemption.
        let m = m.exempting(vec!["hush-crate".into()]);
        assert!(m.hit("dep hush-crate = {"));
        assert_eq!(
            m.term_hits("x Seed::hush() y"),
            BTreeSet::from(["hush".to_string()])
        );
    }

    #[test]
    fn base_phrases_hide_only_themselves_in_any_spelling() {
        let terms = vec!["hush".to_string(), "agent".to_string()];
        let plain = Matcher::new(vec!["AgentPanel".into()], terms.clone());
        let m = plain
            .clone()
            .base_phrases(&["hush fog".into(), "nav agent".into()]);
        for base in [
            "hush fog over the valley",
            "Hush fog",
            "struct HushFog;",
            "let hush_fog = 1;",
            "a hush-fog pass",
            "HUSH_FOG_DENSITY",
            "pub struct NavAgent { radius: f64 }",
            "nav agents avoid each other",
            "`nav_agent::steer`",
        ] {
            // Positive control: without the phrases every one of these is a hit.
            assert!(
                plain.hit(base),
                "{base:?} must hit without the base phrases"
            );
            assert!(!m.hit(base), "{base:?} is base");
        }
        // Every other use of the word is still a term, the same line included.
        for premium in [
            "the hush toggle",
            "hush fog and the hush toggle",
            "HushFogHush",
            "an agent session",
            "NavAgent and the agent protocol",
            "AgentPanels",
            "unnav agent",
            "hush  fog",
        ] {
            assert!(m.hit(premium), "{premium:?} must still hit");
        }
        // The phrase must start a word; the hard identifiers see through it.
        let hush_fog = ["hush".to_string(), "fog".to_string()];
        assert!(phrase_spans("xhush fog", &hush_fog).is_empty());
        assert_eq!(phrase_spans("xHush fog", &hush_fog), vec![(1, 9)]);
        assert!(m.hit("the AgentPanel of nav agents"));
        assert_eq!(
            phrase_spans("a NavAgent b", &["nav".into(), "agent".into()]),
            vec![(2, 10)]
        );
        assert_eq!(
            phrase_spans("é nav-agent", &["nav".into(), "agent".into()]),
            vec![(3, 12)]
        );
    }

    #[test]
    fn hard_hits_lists_every_identifier() {
        let m = Matcher::new(vec!["Alpha".into(), "beta-x".into()], vec![]);
        let hits = m.hard_hits("Alpha and beta-x and Alphas");
        assert_eq!(hits.len(), 2);
        assert!(m.hard_hits("alpha beta").is_empty());
    }
}
