//! Cutting premium material out of files for the public export.
//!
//! **Markdown** ([`scrub_markdown`]), in this order:
//!
//! 1. Manifest rewrites (exact text replacements, `premium.ron` `rewrites`).
//! 2. Marked sections: `<!-- premium:begin <id> -->` ... `<!-- premium:end <id> -->` go,
//!    markers included. A marker the manifest does not list is an error.
//! 3. Headings that name a premium identifier or doc term go **with their section** (down to
//!    the next heading of the same or a higher level). A heading that only *cites* something
//!    cut (an ADR, a chapter, a DoD id that is not exported) keeps its text, minus the
//!    citation.
//! 4. Every remaining block is scrubbed by whole units: a table row, a line of a plain code
//!    block (a code listing goes whole), or a **sentence** of a paragraph, list item or quote
//!    — never a clause out of a sentence, which leaves a fragment. A unit goes when it names
//!    a premium identifier or doc term, or cites something cut (a dangling reference); a
//!    citation that only stands in a list (`Plan references: Ch.2, Ch.11`) is stripped
//!    instead. So that what stays reads as written ([`block_verdict`]): a paragraph or item
//!    whose first sentence goes, goes whole, with its sub-items; a sentence that leans on a
//!    cut one ("This ...", "It ...", a lower-case start) goes with it; a bold lead left with
//!    none of its explanation goes. A kept ordered item after a cut one keeps its number. A
//!    table whose body rows all go goes; a heading left with an empty section goes. Where
//!    base content needs more than that, the source is split so its premium part stands
//!    alone, marked as a premium section (step 2), or restated by a rewrite (step 1). DoD
//!    status reasons, lists of evidence claims, lose their premium claims one by one
//!    ([`scrub_evidence`]).
//! 5. Links ([`fix_links`], run once every file is scrubbed): a link to a file that is not
//!    exported becomes plain text; a link to a heading that was cut loses its anchor.
//!
//! **Row files** ([`cut_ron_rows`]): `dod-status.ron` and `tests/gates.ron` rows are cut by id,
//! with the comment lines directly above them.
//!
//! **The workspace manifest** ([`strip_workspace_manifest`]): premium `[profile.*.package.*]`
//! tables, `[workspace.dependencies]` entries and explicit `members` go, with the comments
//! directly above them.

use std::collections::{BTreeMap, BTreeSet};

use super::scan::{Matcher, id_ranges, names, split_id};

/// References to things that are not exported: a citation of one is a dangling reference.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dangling {
    /// Master-plan chapters cut whole.
    pub chapters: BTreeSet<u32>,
    /// Master-plan sections `(chapter, section)` cut.
    pub sections: BTreeSet<(u32, u32)>,
    /// ADR numbers whose file is not exported.
    pub adrs: BTreeSet<u32>,
    /// DoD ids that are not exported.
    pub dod: BTreeSet<String>,
    /// Files that are not exported although their names are not premium (withheld records):
    /// a DoD row's evidence citing one drops it.
    pub files: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ref {
    Adr(u32),
    Chapter(u32, Option<u32>),
    Dod(String),
    /// A DoD id range (`M7-35..M7-39`) spanning one that is not exported.
    DodRange,
}

fn digits(s: &str) -> (Option<u32>, usize) {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    (s[..n].parse().ok(), n)
}

impl Dangling {
    pub fn is_empty(&self) -> bool {
        self.chapters.is_empty()
            && self.sections.is_empty()
            && self.adrs.is_empty()
            && self.dod.is_empty()
    }

    fn dangles(&self, r: &Ref) -> bool {
        match r {
            Ref::Adr(n) => self.adrs.contains(n),
            Ref::Chapter(c, s) => {
                self.chapters.contains(c) || s.is_some_and(|s| self.sections.contains(&(*c, s)))
            }
            Ref::Dod(id) => self.dod.contains(id),
            Ref::DodRange => true,
        }
    }

    /// Every citation in `text`: `(start, end, reference)`, byte offsets.
    fn refs(&self, text: &str) -> Vec<(usize, usize, Ref)> {
        let mut out = Vec::new();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < text.len() {
            if !text.is_char_boundary(i) {
                i += 1;
                continue;
            }
            let rest = &text[i..];
            let word_start = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric());
            // ADR 0052, ADR-0052
            if word_start && rest.starts_with("ADR") {
                let after = &rest[3..];
                let skip = usize::from(after.starts_with(' ') || after.starts_with('-'));
                let (n, len) = digits(&after[skip..]);
                if let (Some(n), 4) = (n, len) {
                    let end = i + 3 + skip + len;
                    out.push((i, end, Ref::Adr(n)));
                    i = end;
                    continue;
                }
            }
            // Ch.15, Ch. 15, Ch.15.3, §15, §15.3, Chapter 15
            let lead = if word_start && rest.starts_with("Ch.") {
                Some(3 + usize::from(rest[3..].starts_with(' ')))
            } else if rest.starts_with('§') {
                Some('§'.len_utf8() + usize::from(rest['§'.len_utf8()..].starts_with(' ')))
            } else if word_start && rest.starts_with("Chapter ") {
                Some(8)
            } else {
                None
            };
            if let Some(lead) = lead {
                let (c, len) = digits(&rest[lead..]);
                if let Some(c) = c {
                    let mut end = i + lead + len;
                    let mut sec = None;
                    if text[end..].starts_with('.') {
                        let (s, slen) = digits(&text[end + 1..]);
                        if let Some(s) = s {
                            sec = Some(s);
                            end += 1 + slen;
                        }
                    }
                    out.push((i, end, Ref::Chapter(c, sec)));
                    i = end;
                    continue;
                }
            }
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
        for id in &self.dod {
            for (s, _) in text.match_indices(id.as_str()) {
                if names(&text[s..], id) && (s == 0 || !text[..s].ends_with(char::is_alphanumeric))
                {
                    let start = if text[..s].ends_with("DoD ") {
                        s - 4
                    } else {
                        s
                    };
                    out.push((start, s + id.len(), Ref::Dod(id.clone())));
                }
            }
        }
        if !self.dod.is_empty() && text.contains("..") {
            for (s, e, prefix, lo, hi) in id_ranges(text) {
                let spans_one = self.dod.iter().any(|id| {
                    split_id(id).is_some_and(|(p, n)| p == prefix && (lo..=hi).contains(&n))
                });
                if spans_one {
                    let start = if text[..s].ends_with("DoD ") {
                        s - 4
                    } else {
                        s
                    };
                    // The range's own ends are ids too: the range replaces them.
                    out.retain(|(a, b, _)| *b <= start || *a >= e);
                    out.push((start, e, Ref::DodRange));
                }
            }
        }
        out.sort_by_key(|(s, _, _)| *s);
        out
    }

    /// The first dangling citation in `text`, for messages.
    pub fn first(&self, text: &str) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        self.refs(text)
            .into_iter()
            .find(|(_, _, r)| self.dangles(r))
            .map(|(s, e, _)| text[s..e].to_string())
    }

    /// The dangling citations in `text`, byte spans; citations separated only by spaces
    /// (`Ch.11 §11.1`) are one.
    fn spans(&self, text: &str) -> Vec<(usize, usize)> {
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for (s, e, r) in self.refs(text) {
            if !self.dangles(&r) {
                continue;
            }
            match spans.last_mut() {
                Some(last) if last.1 <= s && text[last.1..s].trim().is_empty() => last.1 = e,
                _ => spans.push((s, e)),
            }
        }
        spans
    }

    /// `text` with its dangling citations removed, when every one of them stands in a list of
    /// citations (after `(`, `,`, `;`, `:` or the start, before `,`, `;`, `)`, `.` or the
    /// end): `- **Plan references:** Ch.2, Ch.11, I1` loses `Ch.11` and stays. `None` when a
    /// dangling citation is part of a sentence ("as Ch.11 describes"), or when nothing but a
    /// label would be left: then the whole unit goes.
    pub fn strip_listed(&self, text: &str) -> Option<String> {
        let spans = self.spans(text);
        let listed = spans.iter().all(|(s, e)| {
            let before = text[..*s].trim_end().trim_end_matches('*');
            let after = text[*e..].trim_start();
            (before.is_empty() || before.ends_with(['(', ',', ';', ':', '|']))
                && (after.is_empty() || after.starts_with([',', ';', ')', '.', '|']))
        });
        if !listed {
            return None;
        }
        let out = self.strip(text);
        let ends_in_colon = |t: &str| {
            t.trim_end()
                .trim_end_matches(['*', '.'])
                .trim_end()
                .ends_with(':')
        };
        // A label left with nothing after it (a text that ends in `:` only because its
        // citations went).
        let label_only = ends_in_colon(&out) && !ends_in_colon(text);
        (!label_only && !out.trim().is_empty()).then_some(out)
    }

    /// `text` with every dangling citation removed, with the separator next to it, and any
    /// bracket the removal emptied.
    pub fn strip(&self, text: &str) -> String {
        let spans = self.spans(text);
        if spans.is_empty() {
            return text.to_string();
        }
        let mut out = String::new();
        let mut last = 0;
        for (s, e) in spans {
            if s < last {
                continue;
            }
            out.push_str(&text[last..s]);
            last = e;
            // Drop the separator on one side.
            let tail = &text[e..];
            let before = out.trim_end_matches(' ');
            if before.ends_with([',', ';']) {
                out.truncate(before.len() - 1);
            } else if let Some(n) = [", ", "; ", ",", ";"]
                .iter()
                .find(|sep| tail.starts_with(**sep))
                .map(|sep| sep.len())
            {
                last += n;
            }
        }
        out.push_str(&text[last..]);
        let mut s = out;
        for (from, to) in [
            ("( ", "("),
            (" )", ")"),
            ("()", ""),
            ("(, ", "("),
            ("  ", " "),
        ] {
            while s.contains(from) {
                s = s.replace(from, to);
            }
        }
        s.trim_end().to_string()
    }
}

/// What the scrub removes: premium names (hard identifiers and doc terms) and dangling
/// citations.
pub struct Rules<'a> {
    pub matcher: &'a Matcher,
    pub dangling: &'a Dangling,
}

impl Rules<'_> {
    pub fn hit(&self, text: &str) -> bool {
        self.matcher.hit(text) || self.dangling.first(text).is_some()
    }
}

/// Counts for the export's report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub sections: usize,
    pub headings: usize,
    pub units: usize,
}

impl Stats {
    pub fn add(&mut self, o: &Stats) {
        self.sections += o.sections;
        self.headings += o.headings;
        self.units += o.units;
    }
}

/// Apply the manifest's rewrites for one file. Each `from` must occur (a stale rewrite is
/// reported by the boundary check, which calls this with the same list).
pub fn apply_rewrites(text: &str, rewrites: &[(&str, &str)]) -> (String, Vec<String>) {
    let mut s = text.to_string();
    let mut missing = Vec::new();
    for (from, to) in rewrites {
        if s.contains(from) {
            s = s.replace(from, to);
        } else {
            missing.push((*from).to_string());
        }
    }
    (s, missing)
}

/// Remove the marked premium sections. `known` is the manifest's ids for this file. Returns
/// the text and the ids found; an unknown, nested or unclosed marker is an error.
pub fn cut_marked(text: &str, known: &BTreeSet<String>) -> Result<(String, Vec<String>), String> {
    let mut out = String::with_capacity(text.len());
    let mut open: Option<String> = None;
    let mut found = Vec::new();
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if let Some(id) = t
            .strip_prefix("<!-- premium:begin ")
            .and_then(|r| r.strip_suffix("-->"))
        {
            let id = id.trim().to_string();
            if let Some(o) = &open {
                return Err(format!("premium section {id} opens inside {o}"));
            }
            if !known.contains(&id) {
                return Err(format!(
                    "premium section marker {id} is not listed in premium.ron doc_sections"
                ));
            }
            found.push(id.clone());
            open = Some(id);
            continue;
        }
        if let Some(id) = t
            .strip_prefix("<!-- premium:end ")
            .and_then(|r| r.strip_suffix("-->"))
        {
            let id = id.trim();
            if open.as_deref() != Some(id) {
                return Err(format!("premium:end {id} does not close an open section"));
            }
            open = None;
            continue;
        }
        if open.is_none() {
            out.push_str(line);
        }
    }
    if let Some(o) = open {
        return Err(format!("premium section {o} is never closed"));
    }
    Ok((out, found))
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let n = line.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&n) && line[n..].starts_with(' ') {
        Some((n, line[n..].trim()))
    } else {
        None
    }
}

fn fence(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with("```") {
        Some("```")
    } else if t.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// Numbers a master-plan heading carries: `# Chapter 15 — ...` gives `(15, None)`,
/// `## 15.3 ...` gives `(15, Some(3))`.
pub fn heading_number(text: &str) -> Option<(u32, Option<u32>)> {
    if let Some(rest) = text.strip_prefix("Chapter ") {
        let (c, _) = digits(rest);
        return c.map(|c| (c, None));
    }
    let (c, len) = digits(text);
    let c = c?;
    let rest = &text[len..];
    let rest = rest.strip_prefix('.')?;
    let (s, _) = digits(rest);
    s.map(|s| (c, Some(s)))
}

/// Remove every heading that names a premium thing, with its section. Returns the text and
/// the removed headings' texts.
pub fn cut_headings(text: &str, matcher: &Matcher) -> (String, Vec<String>) {
    let mut out = Vec::new();
    let mut cut = Vec::new();
    let mut skip_level: Option<usize> = None;
    let mut in_fence: Option<&str> = None;
    for line in text.lines() {
        if let Some(f) = in_fence {
            if line.trim_start().starts_with(f) {
                in_fence = None;
            }
            if skip_level.is_none() {
                out.push(line);
            }
            continue;
        }
        if let Some(f) = fence(line) {
            in_fence = Some(f);
            if skip_level.is_none() {
                out.push(line);
            }
            continue;
        }
        if let Some((level, htext)) = heading(line) {
            if skip_level.is_some_and(|l| level <= l) {
                skip_level = None;
            }
            if skip_level.is_none() && matcher.hit(htext) {
                skip_level = Some(level);
                cut.push(htext.to_string());
                continue;
            }
            if skip_level.is_some() {
                cut.push(htext.to_string());
            }
        }
        if skip_level.is_none() {
            out.push(line);
        }
    }
    (join_lines(&out), cut)
}

fn join_lines(lines: &[&str]) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

fn list_item(line: &str) -> Option<(usize, usize)> {
    // (indent, marker width including the following spaces)
    let indent = line.len() - line.trim_start_matches(' ').len();
    let t = &line[indent..];
    if is_rule(t) {
        return None;
    }
    let marker = if t.starts_with(['-', '*', '+']) {
        1
    } else {
        let n = t.bytes().take_while(u8::is_ascii_digit).count();
        if n > 0 && n < 4 && t[n..].starts_with(['.', ')']) {
            n + 1
        } else {
            return None;
        }
    };
    let after = &t[marker..];
    let spaces = after.len() - after.trim_start_matches(' ').len();
    (spaces > 0).then_some((indent, marker + spaces))
}

fn block_start(line: &str) -> bool {
    let t = line.trim_start();
    heading(t).is_some()
        || fence(line).is_some()
        || t.starts_with('|')
        || t.starts_with('>')
        || t.starts_with("<!--")
        || list_item(line).is_some()
}

const ABBREVIATIONS: [&str; 14] = [
    "e.g", "i.e", "vs", "etc", "cf", "ch", "no", "approx", "incl", "fig", "viz", "resp", "min",
    "max",
];

/// Split prose into sentences (slices of `text`, whitespace between them dropped).
pub fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut k = 0;
    while k < chars.len() {
        let (i, c) = chars[k];
        if matches!(c, '.' | '!' | '?') {
            // Closing marks that belong to the sentence.
            let mut j = k + 1;
            while j < chars.len() && matches!(chars[j].1, '*' | '_' | ')' | '"' | '\'' | '`' | ']')
            {
                j += 1;
            }
            let mut w = j;
            while w < chars.len() && chars[w].1.is_whitespace() {
                w += 1;
            }
            let next_ok = w < chars.len()
                && w > j
                && (chars[w].1.is_uppercase()
                    || chars[w].1.is_ascii_digit()
                    || matches!(chars[w].1, '*' | '`' | '(' | '[' | '"' | '_' | '>' | '\''));
            let word: String = text[start..i]
                .rsplit(char::is_whitespace)
                .next()
                .unwrap_or("")
                .trim_start_matches(['(', '*', '_', '`', '"'])
                .to_lowercase();
            let abbrev = c == '.' && ABBREVIATIONS.contains(&word.as_str());
            if next_ok && !abbrev {
                let end = chars.get(j).map_or(text.len(), |(b, _)| *b);
                out.push(text[start..end].trim());
                start = chars[w].0;
                k = w;
                continue;
            }
        }
        k += 1;
    }
    let last = text[start..].trim();
    if !last.is_empty() {
        out.push(last);
    }
    out
}

enum Prose {
    Unchanged,
    Rewritten(String),
    Dropped,
}

/// What happens to a paragraph or list item.
enum Verdict {
    Keep,
    /// Restated without its premium sentences, or without citations of cut things that
    /// stood in lists ([`Dangling::strip_listed`]).
    Restated(String),
    Drop,
}

/// Words that make a sentence lean on the one before it: when that one is cut, this one
/// goes too, or it would point at nothing.
const REFERS_BACK: [&str; 25] = [
    "this",
    "that",
    "these",
    "those",
    "it",
    "its",
    "it's",
    "they",
    "their",
    "them",
    "such",
    "both",
    "so",
    "thus",
    "hence",
    "otherwise",
    "instead",
    "but",
    "and",
    "also",
    "then",
    "however",
    "which",
    "likewise",
    "similarly",
];

/// Does sentence `s` lean on the sentence before it: an anaphoric or connective first word,
/// or a lower-case start (the rest of a sentence)?
fn refers_back(s: &str) -> bool {
    let t = s.trim_start_matches(['*', '_', '(', '"', '>', ' ']);
    // A sentence may open with code (`x` is ...): that is a name, not a continuation.
    if t.starts_with(|c: char| c.is_lowercase()) {
        return true;
    }
    let word: String = t
        .split(char::is_whitespace)
        .next()
        .unwrap_or("")
        .trim_end_matches([',', ':', ';', '*', '_'])
        .to_lowercase();
    REFERS_BACK.contains(&word.as_str())
}

/// Is `s` a bold lead (`**Promote is a command.**`)?
fn bold_lead(s: &str) -> bool {
    let t = s.trim();
    t.starts_with("**") && (t.ends_with("**") || t.ends_with("**:"))
}

/// A paragraph or list item (its text joined into one line), scrubbed by whole sentences:
///
/// * a citation of something cut that only stands in a list is stripped first
///   (`Plan references: Ch.2, Ch.11` keeps `Ch.2`);
/// * a unit whose **first** sentence names a premium thing goes whole: the first sentence
///   says what the unit is about, the rest explains it;
/// * otherwise each sentence that names one goes, whole (never a clause out of it: that
///   leaves a fragment), and a sentence that leans on a cut one ("This ...", "It ...", a
///   lower-case start) goes with it;
/// * a bold lead left with none of its explanation goes too.
fn block_verdict(joined: &str, rules: &Rules<'_>, stats: &mut Stats) -> Verdict {
    if !rules.hit(joined) {
        return Verdict::Keep;
    }
    let listed = rules
        .dangling
        .first(joined)
        .and_then(|_| rules.dangling.strip_listed(joined));
    let text = listed.as_deref().unwrap_or(joined);
    if !rules.hit(text) {
        stats.units += 1;
        return Verdict::Restated(text.to_string());
    }
    let all = sentences(text);
    if all.first().is_none_or(|s| rules.hit(s)) {
        stats.units += 1;
        return Verdict::Drop;
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut prev_cut = false;
    for s in &all {
        prev_cut = rules.hit(s) || (prev_cut && refers_back(s));
        if prev_cut {
            stats.units += 1;
        } else {
            kept.push(s);
        }
    }
    if kept.len() == 1 && all.len() > 1 && bold_lead(kept[0]) {
        return Verdict::Drop;
    }
    Verdict::Restated(kept.join(" "))
}

const WRAP: usize = 92;

fn wrap(text: &str, first: &str, rest: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = first.to_string();
    let mut empty = true;
    for word in text.split_whitespace() {
        if !empty && cur.chars().count() + 1 + word.chars().count() > WRAP {
            lines.push(std::mem::replace(&mut cur, rest.to_string()));
            empty = true;
        }
        if !empty {
            cur.push(' ');
        }
        cur.push_str(word);
        empty = false;
    }
    lines.push(cur);
    lines
}

/// Scrub a DoD status reason. Paragraphs and list items in markdown go whole when they name
/// a premium thing ([`scrub_blocks`]); a status reason is different, a list of evidence
/// claims that each stand alone, so its premium sentences, or `;`-separated claims, go one
/// by one and the rest stays.
fn scrub_evidence(text: &str, rules: &Rules<'_>, stats: &mut Stats) -> Prose {
    if !rules.hit(text) {
        return Prose::Unchanged;
    }
    let mut kept: Vec<String> = Vec::new();
    for s in sentences(text) {
        if !rules.hit(s) {
            kept.push(s.to_string());
            continue;
        }
        stats.units += 1;
        let clean: Vec<&str> = s.split("; ").filter(|c| !rules.hit(c)).collect();
        if clean.is_empty() {
            continue;
        }
        let mut joined = clean.join("; ");
        if let Some(end) = s.chars().last().filter(|c| matches!(c, '.' | '!' | '?'))
            && !joined.ends_with(end)
        {
            joined = joined.trim_end_matches([',', ';', ':']).to_string();
            joined.push(end);
        }
        kept.push(joined);
    }
    if kept.is_empty() {
        Prose::Dropped
    } else {
        Prose::Rewritten(kept.join(" "))
    }
}

/// Scrub the blocks of `text` (steps 3's citation strip on headings and step 4).
pub fn scrub_blocks(text: &str, rules: &Rules<'_>, stats: &mut Stats) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    let mut drop_below: Option<usize> = None;
    // Ordered-list numbering: a kept item after a cut one starts a new list after an empty
    // comment, so it renders under its own number (decisions are cited by number).
    // Per indent: (an item was kept, an item went since).
    let mut numbered: BTreeMap<usize, (bool, bool)> = BTreeMap::new();
    while i < lines.len() {
        let line = lines[i];
        let indent = line.len() - line.trim_start().len();
        if let Some(d) = drop_below {
            if line.trim().is_empty() {
                // A blank line ends the dropped item's children only if what follows is not
                // indented under it.
                let next = lines[i + 1..].iter().find(|l| !l.trim().is_empty());
                if next.is_none_or(|n| n.len() - n.trim_start().len() <= d) {
                    drop_below = None;
                }
                out.push(String::new());
                i += 1;
                continue;
            }
            if indent > d {
                i += 1;
                continue;
            }
            drop_below = None;
        }
        if line.trim().is_empty() {
            out.push(String::new());
            i += 1;
            continue;
        }
        if list_item(line).is_none() {
            // Any other block ends the lists it is not indented under.
            numbered.retain(|ind, _| *ind < indent);
        }
        if let Some(f) = fence(line) {
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim_start().starts_with(f) {
                j += 1;
            }
            let end = j.min(lines.len() - 1);
            let inner = &lines[i + 1..end.max(i + 1)];
            // A code listing (a fence with a language) goes whole when any line of it is
            // premium: half a type or half a function misleads. A plain block (a tree, a
            // transcript) loses only its premium lines.
            let lang = line.trim_start().trim_start_matches(['`', '~']).trim();
            let listing = !lang.is_empty() && lang != "text";
            if rules.hit(line) || (listing && inner.iter().any(|l| rules.hit(l))) {
                stats.units += 1;
            } else {
                out.push(line.to_string());
                for l in &lines[i + 1..end.max(i + 1)] {
                    if rules.hit(l) {
                        stats.units += 1;
                    } else {
                        out.push((*l).to_string());
                    }
                }
                if j < lines.len() {
                    out.push(lines[j].to_string());
                }
            }
            i = j + 1;
            continue;
        }
        if let Some((level, htext)) = heading(line) {
            let stripped = rules.dangling.strip(htext);
            if rules.matcher.hit(&stripped) {
                stats.units += 1;
            } else {
                out.push(format!("{} {stripped}", "#".repeat(level)));
            }
            i += 1;
            continue;
        }
        let t = line.trim_start();
        if t.starts_with('|') {
            let mut j = i;
            while j < lines.len() && lines[j].trim_start().starts_with('|') {
                j += 1;
            }
            let table = &lines[i..j];
            let has_sep = table.len() > 1 && table[1].contains("---");
            let (head, body) = if has_sep {
                table.split_at(2)
            } else {
                table.split_at(0)
            };
            if head.iter().any(|l| rules.hit(l)) {
                stats.units += table.len();
            } else {
                // A row goes when it names a premium thing or cites a cut one in its prose; a
                // citation that is a cell or a list in a cell is stripped instead.
                let kept: Vec<String> = body
                    .iter()
                    .filter_map(|l| {
                        if !rules.hit(l) {
                            Some((*l).to_string())
                        } else if rules.matcher.hit(l) {
                            None
                        } else {
                            rules.dangling.strip_listed(l)
                        }
                    })
                    .collect();
                stats.units += body.len() - kept.len();
                if !(kept.is_empty() && !body.is_empty()) {
                    out.extend(head.iter().map(|l| (*l).to_string()));
                    out.extend(kept);
                }
            }
            i = j;
            continue;
        }
        if t.starts_with('>') {
            let mut j = i;
            while j < lines.len() && lines[j].trim_start().starts_with('>') {
                j += 1;
            }
            let inner: Vec<&str> = lines[i..j]
                .iter()
                .map(|l| {
                    let s = l.trim_start().trim_start_matches('>');
                    s.strip_prefix(' ').unwrap_or(s)
                })
                .collect();
            let inner_text = join_lines(&inner);
            let scrubbed = scrub_blocks(&inner_text, rules, stats);
            let body: Vec<&str> = scrubbed.lines().collect();
            if body.iter().any(|l| !l.trim().is_empty()) {
                let pad = &line[..indent];
                for l in trim_blank_edges(&body) {
                    if l.is_empty() {
                        out.push(format!("{pad}>"));
                    } else {
                        out.push(format!("{pad}> {l}"));
                    }
                }
            }
            i = j;
            continue;
        }
        if let Some((item_indent, marker_w)) = list_item(line) {
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim().is_empty() && !block_start(lines[j]) {
                j += 1;
            }
            let content: Vec<&str> = std::iter::once(&line[item_indent + marker_w..])
                .chain(lines[i + 1..j].iter().map(|l| l.trim()))
                .collect();
            let joined = content.join(" ");
            let verdict = block_verdict(&joined, rules, stats);
            numbered.retain(|ind, _| *ind <= item_indent);
            if digits(&line[item_indent..]).0.is_some() {
                let kept = numbered.entry(item_indent).or_default();
                if matches!(verdict, Verdict::Drop) {
                    kept.1 |= kept.0;
                } else {
                    if kept.1 {
                        let pad = &line[..item_indent];
                        out.extend([String::new(), format!("{pad}<!-- -->"), String::new()]);
                    }
                    *kept = (true, false);
                }
            }
            match verdict {
                Verdict::Keep => out.extend(lines[i..j].iter().map(|l| (*l).to_string())),
                Verdict::Restated(s) => {
                    let first = &line[..item_indent + marker_w];
                    let rest = " ".repeat(item_indent + marker_w);
                    out.extend(wrap(&s, first, &rest));
                }
                // The item goes whole, with its sub-items.
                Verdict::Drop => drop_below = Some(item_indent),
            }
            i = j;
            continue;
        }
        let mut j = i + 1;
        while j < lines.len() && !lines[j].trim().is_empty() && !block_start(lines[j]) {
            j += 1;
        }
        let joined = lines[i..j]
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        let pad = &line[..indent];
        match block_verdict(&joined, rules, stats) {
            Verdict::Keep => out.extend(lines[i..j].iter().map(|l| (*l).to_string())),
            Verdict::Restated(s) if !t.starts_with('<') => out.extend(wrap(&s, pad, pad)),
            _ => {}
        }
        i = j;
    }
    tidy(out)
}

fn trim_blank_edges<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    let start = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |e| e + 1);
    lines[start..end.max(start)].to_vec()
}

fn is_rule(l: &str) -> bool {
    let t = l.trim();
    t.len() >= 3 && (t.bytes().all(|b| b == b'-') || t.bytes().all(|b| b == b'*'))
}

/// Collapse blank runs, repeated rules, and headings left with nothing under them.
fn tidy(lines: Vec<String>) -> String {
    // Headings with an empty section (no text before the next heading of the same or a
    // higher level), innermost first.
    let mut keep: Vec<Option<String>> = lines.into_iter().map(Some).collect();
    let mut in_fence = false;
    let fenced: Vec<bool> = keep
        .iter()
        .map(|l| {
            let l = l.as_deref().unwrap_or("");
            let was = in_fence;
            if fence(l).is_some() {
                in_fence = !in_fence;
                return true;
            }
            was
        })
        .collect();
    for i in (0..keep.len()).rev() {
        let Some(level) = keep[i]
            .as_deref()
            .filter(|_| !fenced[i])
            .and_then(heading)
            .map(|(l, _)| l)
        else {
            continue;
        };
        let mut empty = true;
        for j in i + 1..keep.len() {
            let Some(l) = keep[j].as_deref() else {
                continue;
            };
            if !fenced[j]
                && let Some((l2, _)) = heading(l)
            {
                if l2 <= level {
                    break;
                }
                empty = false;
                break;
            }
            if !l.trim().is_empty() && !is_rule(l) {
                empty = false;
                break;
            }
        }
        if empty && i > 0 {
            keep[i] = None;
        }
    }
    let mut out: Vec<String> = Vec::new();
    for l in keep.into_iter().flatten() {
        let blank = l.trim().is_empty();
        if blank && out.last().is_none_or(|p| p.trim().is_empty()) {
            continue;
        }
        if is_rule(&l) {
            // A rule directly after another rule (blank lines between) is a duplicate.
            let prev = out.iter().rev().find(|p| !p.trim().is_empty());
            if prev.is_some_and(|p| is_rule(p)) || prev.is_none() {
                continue;
            }
        }
        out.push(l);
    }
    while out
        .last()
        .is_some_and(|l| l.trim().is_empty() || is_rule(l))
    {
        out.pop();
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// The whole markdown pipeline except links (steps 1-4).
pub fn scrub_markdown(
    text: &str,
    rewrites: &[(&str, &str)],
    sections: &BTreeSet<String>,
    rules: &Rules<'_>,
) -> Result<(String, Stats), String> {
    let (text, missing) = apply_rewrites(text, rewrites);
    if let Some(m) = missing.first() {
        return Err(format!("rewrite text not found: {m:?}"));
    }
    let (text, found) = cut_marked(&text, sections)?;
    let (text, cut) = cut_headings(&text, rules.matcher);
    let mut stats = Stats {
        sections: found.len(),
        headings: cut.len(),
        units: 0,
    };
    let text = scrub_blocks(&text, rules, &mut stats);
    Ok((text, stats))
}

/// GitHub's heading anchors for a markdown text (duplicates get `-1`, `-2`, ...).
pub fn anchors(text: &str) -> BTreeSet<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = BTreeSet::new();
    let mut in_fence: Option<&str> = None;
    for line in text.lines() {
        if let Some(f) = in_fence {
            if line.trim_start().starts_with(f) {
                in_fence = None;
            }
            continue;
        }
        if let Some(f) = fence(line) {
            in_fence = Some(f);
            continue;
        }
        let Some((_, h)) = heading(line) else {
            continue;
        };
        let slug: String = h
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
            .map(|c| if c == ' ' { '-' } else { c })
            .collect();
        let n = seen.entry(slug.clone()).or_insert(0);
        let slug = if *n == 0 { slug } else { format!("{slug}-{n}") };
        *n += 1;
        out.insert(slug);
    }
    out
}

/// Resolve a link target relative to the file that holds it: `(path, anchor)`, where the
/// path is repo-relative and `/`-separated (empty for a same-file anchor).
pub fn resolve(file: &str, target: &str) -> Option<(String, Option<String>)> {
    let target = target.split_whitespace().next().unwrap_or("");
    if target.is_empty() || target.contains("://") || target.starts_with("mailto:") {
        return None;
    }
    let (path, anchor) = match target.split_once('#') {
        Some((p, a)) => (p, Some(a.to_string())),
        None => (target, None),
    };
    if path.is_empty() {
        return Some((String::new(), anchor));
    }
    let mut parts: Vec<&str> = if path.starts_with('/') {
        Vec::new()
    } else {
        let mut d: Vec<&str> = file.split('/').collect();
        d.pop();
        d
    };
    for seg in path.trim_start_matches('/').split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    Some((parts.join("/"), anchor))
}

/// Every `[text](target)` outside code in `text`: `(start, end, text, target)`.
fn links(text: &str) -> Vec<(usize, usize, String, String)> {
    let mut out = Vec::new();
    let mut offset = 0;
    let mut in_fence: Option<&str> = None;
    for line in text.split_inclusive('\n') {
        let base = offset;
        offset += line.len();
        if let Some(f) = in_fence {
            if line.trim_start().starts_with(f) {
                in_fence = None;
            }
            continue;
        }
        if let Some(f) = fence(line) {
            in_fence = Some(f);
            continue;
        }
        let b = line.as_bytes();
        let mut i = 0;
        let mut in_code = false;
        while i < b.len() {
            match b[i] {
                b'`' => in_code = !in_code,
                b'[' if !in_code => {
                    // Matching ']' then '(' ... ')'.
                    let mut depth = 0;
                    let mut j = i;
                    let mut close = None;
                    while j < b.len() {
                        match b[j] {
                            b'[' => depth += 1,
                            b']' => {
                                depth -= 1;
                                if depth == 0 {
                                    close = Some(j);
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    if let Some(c) = close
                        && b.get(c + 1) == Some(&b'(')
                    {
                        let mut pd = 0;
                        let mut k = c + 1;
                        let mut end = None;
                        while k < b.len() {
                            match b[k] {
                                b'(' => pd += 1,
                                b')' => {
                                    pd -= 1;
                                    if pd == 0 {
                                        end = Some(k);
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            k += 1;
                        }
                        if let Some(e) = end {
                            out.push((
                                base + i,
                                base + e + 1,
                                line[i + 1..c].to_string(),
                                line[c + 2..e].to_string(),
                            ));
                            i = e + 1;
                            continue;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
    out
}

/// The link targets in `text` that do not resolve: `(target, why)`. `exists` answers for a
/// repo-relative file or directory; `anchors_of` gives an exported markdown file's anchors.
pub fn dangling_links(
    file: &str,
    text: &str,
    exists: &dyn Fn(&str) -> bool,
    anchors_of: &dyn Fn(&str) -> Option<BTreeSet<String>>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (_, _, _, target) in links(text) {
        let Some((path, anchor)) = resolve(file, &target) else {
            continue;
        };
        let path = if path.is_empty() {
            file.to_string()
        } else {
            path
        };
        if !exists(&path) {
            out.push((target, "file not exported".into()));
        } else if let Some(a) = anchor
            && let Some(set) = anchors_of(&path)
            && !set.contains(&a.to_lowercase())
        {
            out.push((target, "anchor not exported".into()));
        }
    }
    out
}

/// Rewrite the links of `text` so none dangles: a link to a missing file (or a missing
/// same-file anchor) becomes its text; a link to a missing anchor in an exported file keeps
/// the file and loses the anchor. Returns the text and how many links changed.
pub fn fix_links(
    file: &str,
    text: &str,
    exists: &dyn Fn(&str) -> bool,
    anchors_of: &dyn Fn(&str) -> Option<BTreeSet<String>>,
) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut changed = 0;
    for (s, e, label, target) in links(text) {
        let Some((path, anchor)) = resolve(file, &target) else {
            continue;
        };
        let same = path.is_empty();
        let path = if same { file.to_string() } else { path };
        let replacement = if !exists(&path) {
            Some(label.clone())
        } else if let Some(a) = &anchor
            && let Some(set) = anchors_of(&path)
            && !set.contains(&a.to_lowercase())
        {
            if same {
                Some(label.clone())
            } else {
                let t = target.split('#').next().unwrap_or("");
                Some(format!("[{label}]({t})"))
            }
        } else {
            None
        };
        if let Some(r) = replacement {
            out.push_str(&text[last..s]);
            out.push_str(&r);
            last = e;
            changed += 1;
        }
    }
    out.push_str(&text[last..]);
    (out, changed)
}

/// Cut rows out of a RON row file (`dod-status.ron`, `tests/gates.ron`): a row starts at a
/// line `(id: "..."` and owns the comment lines directly above it. `drop(id, row_text)`
/// decides. Returns the text and the dropped ids.
pub fn cut_ron_rows(text: &str, drop: &dyn Fn(&str, &str) -> bool) -> (String, Vec<String>) {
    map_ron_rows(text, &|id, row| (!drop(id, row)).then(|| row.to_string()))
}

/// Rewrite the rows of a RON row file: `f(id, row_text)` returns the row's new text, or
/// `None` to drop it. Returns the text and the dropped ids.
pub fn map_ron_rows(text: &str, f: &dyn Fn(&str, &str) -> Option<String>) -> (String, Vec<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let row_id = |l: &str| -> Option<String> {
        let r = l.trim_start().strip_prefix("(id: \"")?;
        Some(r.split('"').next()?.to_string())
    };
    let starts: Vec<usize> = (0..lines.len())
        .filter(|i| row_id(lines[*i]).is_some())
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut dropped = Vec::new();
    let mut next = 0;
    for (k, &s) in starts.iter().enumerate() {
        // The row's own lines: up to the next row's comments, the next row, or the list end.
        let mut end = starts.get(k + 1).copied().unwrap_or(lines.len());
        if k + 1 == starts.len() {
            end = (s + 1..lines.len())
                .find(|j| lines[*j].trim_start().starts_with(']'))
                .unwrap_or(lines.len());
        }
        while end > s + 1 && lines[end - 1].trim_start().starts_with("//") {
            end -= 1;
        }
        let mut begin = s;
        while begin > next && lines[begin - 1].trim_start().starts_with("//") {
            begin -= 1;
        }
        out.extend(lines[next..begin].iter().map(|l| (*l).to_string()));
        let id = row_id(lines[s]).unwrap_or_default();
        match f(&id, &lines[begin..end].join("\n")) {
            Some(t) => out.extend(t.lines().map(str::to_string)),
            None => dropped.push(id),
        }
        next = end;
    }
    out.extend(lines[next..].iter().map(|l| (*l).to_string()));
    let refs: Vec<&str> = out.iter().map(String::as_str).collect();
    (join_lines(&refs), dropped)
}

/// One `dod-status.ron` row as the public edition shows it, or `None` when nothing honest is
/// left: comment lines that name a premium thing go; an evidence path that names one leaves
/// the list (a row left with no evidence goes); a reason or superseded wording is scrubbed
/// sentence by sentence (a row whose reason empties goes).
pub fn scrub_status_row(row: &str, rules: &Rules<'_>) -> Option<String> {
    let mut out = String::with_capacity(row.len());
    let mut stats = Stats::default();
    for line in row.split_inclusive('\n') {
        if line.trim_start().starts_with("//") {
            if !rules.hit(line) {
                out.push_str(line);
            }
            continue;
        }
        let mut rest = line;
        while let Some(q) = rest.find('"') {
            let (head, tail) = rest.split_at(q);
            // The literal: up to the next unescaped quote.
            let body_len = {
                let b = tail.as_bytes();
                let mut j = 1;
                while j < b.len() && !(b[j] == b'"' && b[j - 1] != b'\\') {
                    j += 1;
                }
                j
            };
            let lit = &tail[1..body_len.min(tail.len())];
            let after = &tail[(body_len + 1).min(tail.len())..];
            let key = head.trim_end().trim_end_matches(':');
            let key = key.rsplit([' ', '(', ',']).next().unwrap_or("");
            let in_evidence = {
                let so_far = format!("{out}{head}");
                so_far
                    .rfind("evidence: [")
                    .is_some_and(|e| !so_far[e..].contains(']'))
            };
            if in_evidence {
                if rules.hit(lit) || rules.dangling.files.contains(lit) {
                    // Drop the item and one separator.
                    let trimmed = head.trim_end_matches(", ");
                    out.push_str(if head.ends_with(", ") { trimmed } else { head });
                    rest = if head.ends_with(", ") {
                        after
                    } else {
                        after.strip_prefix(", ").unwrap_or(after)
                    };
                    continue;
                }
            } else if matches!(key, "reason" | "text") {
                let prose = lit.replace("\\\"", "\"");
                let new = match scrub_evidence(&prose, rules, &mut stats) {
                    Prose::Unchanged => prose,
                    Prose::Rewritten(s) => s,
                    Prose::Dropped => return None,
                };
                out.push_str(head);
                out.push('"');
                out.push_str(&new.replace('"', "\\\""));
                out.push('"');
                rest = after;
                continue;
            }
            out.push_str(head);
            out.push('"');
            out.push_str(lit);
            out.push('"');
            rest = after;
        }
        out.push_str(rest);
    }
    if out.contains("evidence: []") || rules.matcher.hit(&out) {
        return None;
    }
    Some(out)
}

/// The workspace `Cargo.toml` without premium members, `[workspace.dependencies]` entries or
/// per-package profile tables (with the comment lines directly above each).
pub fn strip_workspace_manifest(
    text: &str,
    is_premium: &dyn Fn(&str) -> bool,
    premium_paths: &[String],
) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut remove = vec![false; lines.len()];
    let header = |l: &str| -> Option<String> {
        let t = l.trim();
        t.strip_prefix('[')
            .and_then(|r| r.strip_suffix(']'))
            .map(str::to_string)
    };
    let mut section = String::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let mut range: Option<(usize, usize)> = None;
        if let Some(h) = header(l) {
            section = h.clone();
            let pkg = h
                .split_once(".package.")
                .filter(|(p, _)| p.starts_with("profile."))
                .map(|(_, n)| n.trim_matches('"').to_string())
                .or_else(|| {
                    h.strip_prefix("workspace.dependencies.")
                        .map(str::to_string)
                });
            if pkg.is_some_and(|p| is_premium(&p)) {
                let mut j = i + 1;
                while j < lines.len() && header(lines[j]).is_none() && !lines[j].trim().is_empty() {
                    j += 1;
                }
                range = Some((i, j));
            }
        } else if section == "workspace.dependencies"
            && let Some((key, _)) = l.split_once('=')
            && is_premium(key.trim().trim_matches('"'))
        {
            range = Some((i, i + 1));
        }
        if let Some((s, e)) = range {
            let mut b = s;
            while b > 0 && lines[b - 1].trim_start().starts_with('#') {
                b -= 1;
            }
            for r in &mut remove[b..e] {
                *r = true;
            }
            // Drop one blank line the removal would leave doubled.
            if b > 0
                && lines[b - 1].trim().is_empty()
                && lines.get(e).is_some_and(|n| n.trim().is_empty())
            {
                remove[b - 1] = true;
            }
            i = e;
            continue;
        }
        i += 1;
    }
    let mut kept: Vec<String> = lines
        .iter()
        .zip(&remove)
        .filter(|(_, r)| !**r)
        .map(|(l, _)| (*l).to_string())
        .collect();
    // Explicit members.
    for l in &mut kept {
        if l.trim_start().starts_with("members") || l.trim_start().starts_with('"') {
            for p in premium_paths {
                for pat in [
                    format!("\"{p}\", "),
                    format!(", \"{p}\""),
                    format!("\"{p}\","),
                ] {
                    *l = l.replace(&pat, "");
                }
            }
        }
    }
    let refs: Vec<&str> = kept
        .iter()
        .map(String::as_str)
        .filter(|l| !premium_paths.iter().any(|p| l.trim() == format!("\"{p}\"")))
        .collect();
    join_lines(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher() -> Matcher {
        Matcher::new(
            vec![
                "forge-secret".into(),
                "forge_secret".into(),
                "SECRET".into(),
            ],
            vec!["hush".into()],
        )
    }

    fn scrub(text: &str, dangling: &Dangling) -> String {
        let m = matcher();
        let rules = Rules {
            matcher: &m,
            dangling,
        };
        let known: BTreeSet<String> = ["ch-9".to_string()].into();
        scrub_markdown(text, &[], &known, &rules)
            .unwrap_or_else(|e| panic!("{e}"))
            .0
    }

    #[test]
    fn sentences_split_on_ends_but_not_on_abbreviations_or_numbers() {
        let s =
            sentences("One (e.g. two) is 1.5 m. **Bold lead.** Next one! Ch. 5 stays. `x.` Yes.");
        assert_eq!(
            s,
            vec![
                "One (e.g. two) is 1.5 m.",
                "**Bold lead.**",
                "Next one!",
                "Ch. 5 stays.",
                "`x.`",
                "Yes."
            ]
        );
    }

    #[test]
    fn sentences_rows_items_and_code_lines_that_name_premium_go_whole() {
        let src = "\
# Title

Base text. The forge-secret crate is private. More base text.

| a | b |
|---|---|
| keep | row |
| the SECRET | row |

- keep me
- a hushed item
  continued here
  - its child goes too
- kept after

```
keep line
forge_secret line
```

```rust
fn listing() {}
// forge_secret
```
";
        let out = scrub(src, &Dangling::default());
        assert!(out.contains("Base text. More base text."), "{out}");
        assert!(out.contains("| keep | row |"));
        assert!(!out.contains("SECRET") && !out.contains("forge") && !out.contains("hush"));
        assert!(
            !out.contains("its child") && !out.contains("listing"),
            "{out}"
        );
        assert!(out.contains("- kept after") && out.contains("keep line"));
    }

    #[test]
    fn the_scrub_never_leaves_a_fragment_or_a_sentence_pointing_at_nothing() {
        let src = "\
# T

Base one. The SECRET thing is fast. It is also small. Base two.

Base three. A table of numbers; the SECRET row too.

The SECRET opens this paragraph. Base four.

1. **Lead is SECRET.** Everything else explains it.
2. **Base lead.** Base body; with a clause. The SECRET aside.
3. **Bare lead.** Only a SECRET body.
4. Plain base item.
";
        let out = scrub(src, &Dangling::default());
        // Whole sentences go, never a clause; a sentence leaning on a cut one goes with it.
        assert!(
            out.contains("Base one. Base two.") && !out.contains("also small"),
            "{out}"
        );
        assert!(
            !out.contains("A table of numbers") && out.contains("Base three."),
            "{out}"
        );
        // A paragraph or item whose first sentence goes, goes whole; so does a bold lead
        // left with none of its explanation.
        assert!(
            !out.contains("Base four") && !out.contains("Everything else"),
            "{out}"
        );
        assert!(!out.contains("Bare lead"), "{out}");
        assert!(
            out.contains("2. **Base lead.** Base body; with a clause.") && !out.contains("aside"),
            "{out}"
        );
        // A list starting after cut items starts at its own number; a kept item after a cut
        // one keeps its number by starting a list of its own.
        assert!(out.contains("<!-- -->\n\n4. Plain base item."), "{out}");
        let lazy = scrub(
            "# T\n\n1. a\n1. b\n1. c\n\n1. again\n",
            &Dangling::default(),
        );
        assert!(!lazy.contains("<!--"), "{lazy}");
        let first = scrub("# T\n\n1. SECRET\n2. kept\n", &Dangling::default());
        assert!(
            first.contains("\n2. kept") && !first.contains("<!--"),
            "{first}"
        );
        for s in ["It is.", "it goes on.", "However, no.", "*This* one."] {
            assert!(refers_back(s), "{s}");
        }
        for s in ["Base.", "The base.", "`x` is.", "There is one."] {
            assert!(!refers_back(s), "{s}");
        }
    }

    #[test]
    fn citations_in_lists_are_stripped_and_citations_in_sentences_take_the_unit() {
        let d = Dangling {
            chapters: [11].into(),
            adrs: [52].into(),
            ..Dangling::default()
        };
        assert_eq!(
            d.strip_listed("**Plan references:** Ch.2, Ch.11 §11.1, I1")
                .as_deref(),
            Some("**Plan references:** Ch.2, I1")
        );
        assert_eq!(
            d.strip_listed("Base (Ch.2; ADR 0052).").as_deref(),
            Some("Base (Ch.2).")
        );
        assert_eq!(d.strip_listed("As Ch.11 describes, base."), None);
        // A text that introduces a table keeps its colon; a cell or a list in a cell is a
        // list too.
        assert_eq!(
            d.strip_listed("Each item (Ch.2, Ch.11) is classified here:")
                .as_deref(),
            Some("Each item (Ch.2) is classified here:")
        );
        assert_eq!(
            d.strip_listed("| Viewport | Ch.2, Ch.11, Ch.21 | x |")
                .as_deref(),
            Some("| Viewport | Ch.2, Ch.21 | x |")
        );
        let table = scrub(
            "# T\n\n| a | b |\n|---|---|\n| kept | Ch.2, Ch.11 |\n| gone | as Ch.11 says |\n",
            &d,
        );
        assert!(
            table.contains("| kept | Ch.2 |") && !table.contains("gone"),
            "{table}"
        );
        assert_eq!(d.strip_listed("**Plan references:** Ch.11"), None);
        let out = scrub(
            "# T\n\n- **Plan references:** Ch.2, Ch.11\n- Built as Ch.11 says.\n- Base.\n",
            &d,
        );
        assert!(
            out.contains("- **Plan references:** Ch.2\n- Base."),
            "{out}"
        );
    }

    #[test]
    fn a_dod_range_spanning_a_cut_id_dangles() {
        let d = Dangling {
            dod: ["M7-36".to_string()].into(),
            ..Dangling::default()
        };
        assert_eq!(
            d.first("rows M7-35..M7-39 are done"),
            Some("M7-35..M7-39".into())
        );
        assert_eq!(d.first("rows M7-37..M7-39 are done"), None);
        assert_eq!(d.strip("Sky (DoD M7-30..40)"), "Sky");
    }

    #[test]
    fn marked_sections_and_premium_headings_go_with_their_sections() {
        let src = "\
# Doc

## 1.1 Base

base

<!-- premium:begin ch-9 -->
## 9 Marked

gone
<!-- premium:end ch-9 -->

## 2.1 The hush section

gone too

### 2.1.1 child

also gone

## 3.1 Base again

kept
";
        let out = scrub(src, &Dangling::default());
        assert!(!out.contains("gone") && !out.contains("child") && !out.contains("premium:"));
        assert!(out.contains("## 1.1 Base") && out.contains("## 3.1 Base again"));
    }

    #[test]
    fn unknown_or_unclosed_markers_are_errors() {
        let known: BTreeSet<String> = ["a".to_string()].into();
        assert!(cut_marked("<!-- premium:begin b -->\n<!-- premium:end b -->\n", &known).is_err());
        assert!(cut_marked("<!-- premium:begin a -->\nx\n", &known).is_err());
        assert!(cut_marked("<!-- premium:end a -->\n", &known).is_err());
    }

    #[test]
    fn dangling_citations_drop_sentences_and_are_stripped_from_headings() {
        let d = Dangling {
            chapters: [9].into(),
            sections: [(2, 6)].into(),
            adrs: [52].into(),
            dod: ["M3-1".to_string()].into(),
            files: std::collections::BTreeSet::new(),
        };
        assert!(d.first("see Ch.9 for it").is_some());
        assert!(d.first("see Ch.2 §2.6").is_some());
        assert!(d.first("see Ch.2 §2.5 and Ch.19").is_none());
        assert!(d.first("(ADR 0052)").is_some() && d.first("ADR 0051").is_none());
        assert!(d.first("DoD M3-1 here").is_some() && d.first("M3-10").is_none());
        assert_eq!(
            d.strip("10.1 Implementation (WP-09, ADR 0022, ADR 0052)"),
            "10.1 Implementation (WP-09, ADR 0022)"
        );
        assert_eq!(d.strip("11.2 Sky (DoD M3-1; ADR 0052)"), "11.2 Sky");
        let out = scrub(
            "## 5.1 Things (WP-1, ADR 0052)\n\nKept.\n\nCut by Ch.9 citation, with its paragraph.\n\nKept too.\n",
            &d,
        );
        assert!(out.contains("## 5.1 Things (WP-1)"), "{out}");
        assert!(
            out.contains("Kept.\n\nKept too.") && !out.contains("paragraph"),
            "{out}"
        );
    }

    #[test]
    fn headings_left_empty_go() {
        let out = scrub(
            "# Doc\n\nintro\n\n## A\n\nThe SECRET.\n\n## B\n\nkept\n",
            &Dangling::default(),
        );
        assert!(!out.contains("## A") && out.contains("## B"), "{out}");
    }

    #[test]
    fn links_to_missing_files_and_anchors_are_rewritten() {
        let text = "See [plan](../plan/a.md#gone), [adr](0052-x.md), [ok](../plan/a.md#here), [web](https://x.y) and `[code](nope.md)`.\n";
        let exists = |p: &str| p == "docs/plan/a.md";
        let anchors = |p: &str| (p == "docs/plan/a.md").then(|| ["here".to_string()].into());
        assert_eq!(
            dangling_links("docs/adr/x.md", text, &exists, &anchors).len(),
            2
        );
        let (out, n) = fix_links("docs/adr/x.md", text, &exists, &anchors);
        assert_eq!(n, 2);
        assert!(
            out.contains("[plan](../plan/a.md),") && out.contains(" adr,"),
            "{out}"
        );
        assert!(out.contains("[ok](../plan/a.md#here)") && out.contains("`[code](nope.md)`"));
        assert!(dangling_links("docs/adr/x.md", &out, &exists, &anchors).is_empty());
    }

    #[test]
    fn anchors_follow_github() {
        let a = anchors("# Chapter 2 — Coordinates, Frames & Time\n## A\n## A\n```\n# not\n```\n");
        assert!(a.contains("chapter-2--coordinates-frames--time"), "{a:?}");
        assert!(a.contains("a") && a.contains("a-1") && !a.contains("not"));
    }

    #[test]
    fn ron_rows_go_with_their_comments() {
        let src = "(\n    rows: [\n        (id: \"I1\", x: 1),\n        // about I2\n        (id: \"I2\", x: 2,\n            more: 3),\n        (id: \"I3\", x: 3),\n    ],\n)\n";
        let (out, dropped) = cut_ron_rows(src, &|id, _| id == "I2");
        assert_eq!(dropped, vec!["I2".to_string()]);
        assert!(!out.contains("I2") && !out.contains("about") && !out.contains("more"));
        assert!(out.contains("I1") && out.contains("I3") && out.contains("],"));
        let (_, last) = cut_ron_rows(src, &|id, _| id == "I3");
        assert_eq!(last, vec!["I3".to_string()]);
    }

    #[test]
    fn status_rows_lose_premium_evidence_and_sentences_and_go_when_empty() {
        let m = matcher();
        let d = Dangling::default();
        let rules = Rules {
            matcher: &m,
            dangling: &d,
        };
        let row = "        // about forge-secret\n        (id: \"M8-1\", status: Settled(evidence: [\"crates/forge-secret/a.rs\", \"crates/base/b.rs\", \"x/forge_secret.rs\"])),";
        let out = scrub_status_row(row, &rules).unwrap_or_default();
        assert_eq!(
            out,
            "        (id: \"M8-1\", status: Settled(evidence: [\"crates/base/b.rs\"])),"
        );
        let only =
            "        (id: \"M8-2\", status: Settled(evidence: [\"crates/forge-secret/a.rs\"])),";
        assert_eq!(scrub_status_row(only, &rules), None);
        let reason = "        (id: \"M8-3\", status: Blocked(reason: \"Kept. The SECRET part. Also \\\"kept\\\".\")),";
        assert_eq!(
            scrub_status_row(reason, &rules).unwrap_or_default(),
            "        (id: \"M8-3\", status: Blocked(reason: \"Kept. Also \\\"kept\\\".\")),"
        );
        let gone = "        (id: \"M8-4\", status: Blocked(reason: \"All SECRET.\")),";
        assert_eq!(scrub_status_row(gone, &rules), None);
        let src = format!("(\n    items: [\n{row}\n{only}\n{reason}\n    ],\n)\n");
        let (t, dropped) = map_ron_rows(&src, &|_, r| scrub_status_row(r, &rules));
        assert_eq!(dropped, vec!["M8-2".to_string()]);
        assert!(
            t.contains("M8-1") && t.contains("M8-3") && !t.contains("secret"),
            "{t}"
        );
    }

    #[test]
    fn workspace_manifest_loses_premium_tables_and_entries() {
        let src = "\
[workspace]
members = [\"crates/*\", \"crates/forge-secret\"]

[workspace.dependencies]
serde = \"1\"
# the secret one
forge-secret = { path = \"crates/forge-secret\" }

# base profile
[profile.dev.package.forge-num]
opt-level = 2

# why the secret is optimised
[profile.dev.package.forge-secret]
opt-level = 2

[profile.release]
lto = true
";
        let out = strip_workspace_manifest(
            src,
            &|n| n == "forge-secret",
            &["crates/forge-secret".to_string()],
        );
        assert!(!out.contains("secret"), "{out}");
        assert!(out.contains("members = [\"crates/*\"]"), "{out}");
        assert!(out.contains("[profile.dev.package.forge-num]") && out.contains("serde = \"1\""));
        assert!(out.contains("[profile.release]") && out.contains("# base profile"));
    }
}
