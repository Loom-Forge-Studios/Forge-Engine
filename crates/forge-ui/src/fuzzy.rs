//! Fuzzy matching for the command palette, search fields and filtered dropdowns
//! (Ch.21 §21.2: written in-house because `nucleo` is MPL-2.0).
//!
//! A query matches a candidate when its characters appear in order (case-insensitive).
//! The score rewards what people mean when they type an abbreviation: matches at word
//! starts ("op" → **O**pen **P**rofiler), consecutive runs, and a match at the very start;
//! it penalises gaps. Matched character positions are returned for highlighting. The
//! algorithm is a greedy forward scan with a word-start preference, O(len) per candidate,
//! so filtering 10,000 actions per keystroke costs well under a millisecond (release).

/// A match: its score (higher is better) and the matched byte offsets in the candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    pub positions: Vec<usize>,
}

fn is_word_start(prev: Option<char>, c: char) -> bool {
    match prev {
        None => true,
        Some(p) => {
            (!p.is_alphanumeric() && c.is_alphanumeric())
                || (p.is_lowercase() && c.is_uppercase())
                || (p.is_alphabetic() && c.is_ascii_digit())
        }
    }
}

/// Score `candidate` against `query`; `None` if it does not match. An empty query
/// matches everything with score 0.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<Match> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if q.is_empty() {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }
    let chars: Vec<(usize, char)> = candidate.char_indices().collect();
    // Pass 1: prefer word-start matches for each query char when one exists ahead.
    let mut positions = Vec::with_capacity(q.len());
    let mut qi = 0usize;
    let mut i = 0usize;
    while qi < q.len() && i < chars.len() {
        let want = q[qi];
        // Look ahead for a word-start occurrence before any plain occurrence is taken,
        // unless the next char matches directly (keeps runs together).
        let (_, c) = chars[i];
        let lc = c.to_lowercase().next().unwrap_or(c);
        if lc == want {
            let continuing = positions.last().is_some_and(|p: &usize| {
                chars
                    .iter()
                    .position(|(b, _)| b == p)
                    .is_some_and(|pi| pi + 1 == i)
            });
            let here_start = is_word_start(i.checked_sub(1).map(|j| chars[j].1), c);
            if !continuing && !here_start {
                // Is there a word start matching `want` later? Prefer it.
                let later = (i + 1..chars.len()).find(|&j| {
                    let cj = chars[j].1;
                    cj.to_lowercase().next() == Some(want)
                        && is_word_start(Some(chars[j - 1].1), cj)
                });
                if let Some(j) = later {
                    // Only jump if the rest of the query still fits after j.
                    let rest_fits = {
                        let mut k = qi + 1;
                        for &(_, cc) in &chars[j + 1..] {
                            if k < q.len() && cc.to_lowercase().next() == Some(q[k]) {
                                k += 1;
                            }
                        }
                        k == q.len()
                    };
                    if rest_fits {
                        i = j;
                        continue;
                    }
                }
            }
            positions.push(chars[i].0);
            qi += 1;
        }
        i += 1;
    }
    if qi < q.len() {
        return None;
    }
    // Score.
    let mut score = 0i32;
    let mut prev_idx: Option<usize> = None;
    for &p in &positions {
        let idx = chars.iter().position(|(b, _)| *b == p).unwrap_or(0);
        let c = chars[idx].1;
        score += 10;
        if is_word_start(idx.checked_sub(1).map(|j| chars[j].1), c) {
            score += 25;
        }
        if idx == 0 {
            score += 20;
        }
        match prev_idx {
            Some(pi) if pi + 1 == idx => score += 15,
            Some(pi) => score -= (idx - pi - 1).min(10) as i32,
            None => score -= idx.min(15) as i32,
        }
        prev_idx = Some(idx);
    }
    // Shorter candidates win ties.
    score -= (chars.len() / 8) as i32;
    Some(Match { score, positions })
}

/// Filter and rank candidates; ties keep their original order (stable).
pub fn rank<'a, T>(query: &str, items: &'a [T], text: impl Fn(&T) -> &str) -> Vec<(usize, Match)>
where
    T: 'a,
{
    let mut out: Vec<(usize, Match)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| fuzzy_match(query, text(it)).map(|m| (i, m)))
        .collect();
    out.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    out
}

/// A case-insensitive substring query (a filter box), folded **once**: matching a
/// candidate allocates nothing. ASCII query against an ASCII candidate is a byte scan;
/// anything else folds both sides char by char (`char::to_lowercase`), so "ÄPFEL" finds
/// "äpfel" and the Kelvin sign finds "k". Surrounding whitespace in the query is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContainsQuery {
    folded: String,
    ascii: bool,
}

impl ContainsQuery {
    pub fn new(query: &str) -> Self {
        let folded: String = query.trim().chars().flat_map(char::to_lowercase).collect();
        let ascii = folded.is_ascii();
        Self { folded, ascii }
    }
    /// True for an empty (or all-whitespace) query, which matches everything.
    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }
    /// The folded query text.
    pub fn as_str(&self) -> &str {
        &self.folded
    }
    /// The refinement test: `other` narrows `self` (its folded text contains ours), so
    /// every candidate `other` matches is one `self` matches too.
    pub fn is_narrowed_by(&self, other: &ContainsQuery) -> bool {
        other.folded.contains(self.folded.as_str())
    }
    /// Does `candidate` contain the query, ignoring case? Allocation-free.
    pub fn matches(&self, candidate: &str) -> bool {
        let n = self.folded.as_bytes();
        if n.is_empty() {
            return true;
        }
        if self.ascii && candidate.is_ascii() {
            let h = candidate.as_bytes();
            if n.len() > h.len() {
                return false;
            }
            let first = n[0];
            return (0..=h.len() - n.len()).any(|i| {
                h[i].to_ascii_lowercase() == first
                    && h[i + 1..i + n.len()]
                        .iter()
                        .zip(&n[1..])
                        .all(|(a, b)| a.to_ascii_lowercase() == *b)
            });
        }
        candidate.char_indices().any(|(i, _)| {
            let mut hay = candidate[i..].chars().flat_map(char::to_lowercase);
            self.folded.chars().all(|c| hay.next() == Some(c))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matches_and_word_starts_rank_first() {
        assert!(fuzzy_match("opf", "Open: Profiler").is_some());
        assert!(fuzzy_match("xyz", "Open: Profiler").is_none());
        let items = [
            "Toggle Wireframe",
            "Open: Profiler",
            "Save Project",
            "Open Prefab",
        ];
        let r = rank("op", &items, |s| s);
        let names: Vec<&str> = r.iter().map(|(i, _)| items[*i]).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().all(|n| n.starts_with("Open")), "{names:?}");
        let tw = rank("tw", &items, |s| s);
        assert_eq!(items[tw[0].0], "Toggle Wireframe", "initials match");
        let m = fuzzy_match("op", "Open: Profiler").unwrap_or(Match {
            score: 0,
            positions: vec![],
        });
        assert_eq!(m.positions, vec![0, 1], "a prefix run is kept together");
        let w = fuzzy_match("wf", "Toggle Wireframe").unwrap_or(Match {
            score: 0,
            positions: vec![],
        });
        assert_eq!(
            w.positions,
            vec![7, 11],
            "the f of 'frame' is taken, not 'Wireframe's second letter"
        );
    }

    #[test]
    fn contains_query_ignores_case_without_allocating_per_candidate() {
        let q = ContainsQuery::new("  Entity 5 ");
        assert!(q.matches("entity 5.12"));
        assert!(q.matches("Big ENTITY 5"));
        assert!(!q.matches("Entity 4.5"));
        assert!(!q.matches("Ent"));
        assert!(ContainsQuery::new("").matches("anything"));
        assert!(ContainsQuery::new("   ").is_empty());
        // Non-ASCII on either side folds char by char.
        assert!(ContainsQuery::new("ÄPFEL").matches("grüne äpfel"));
        assert!(ContainsQuery::new("k").matches("\u{212a}elvin"));
        assert!(ContainsQuery::new("é").matches("CAFÉ"));
        assert!(!ContainsQuery::new("é").matches("cafe"));
        // Refinement: a longer query narrows a shorter one it contains.
        assert!(ContainsQuery::new("ent").is_narrowed_by(&ContainsQuery::new("Entity")));
        assert!(!ContainsQuery::new("entity").is_narrowed_by(&ContainsQuery::new("ent")));
    }

    #[test]
    fn empty_query_matches_all_in_order() {
        let items = ["b", "a"];
        let r = rank("", &items, |s| s);
        assert_eq!(r.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0, 1]);
    }
}
