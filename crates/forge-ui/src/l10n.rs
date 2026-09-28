//! Localisation primitives (Ch.28; Ch.21 §21.20, §21.21 "Localisation") shared by the
//! editor's string-table editor and shipped game UI — one implementation, so what the editor
//! previews is exactly what the game shows.
//!
//! * **Templates.** A string may carry named placeholders, `{name}`; `{{` and `}}` are
//!   literal braces. [`placeholders`] lists them, [`format`] fills them, and a translation
//!   that loses or invents one is a problem the editor reports ([`placeholder_mismatch`]).
//! * **Pseudo-localisation** ([`pseudo_localise`], the pseudo-locale [`PSEUDO_LOCALE`]).
//!   Every letter is swapped for an accented look-alike (so a string that was *not* looked
//!   up — a hard-coded literal — stands out untransformed), the text is lengthened by about
//!   40 % (so layouts that only fit English break in the editor, not in a customer's German
//!   build), and it is bracketed (so truncation at either end is visible). Placeholders
//!   survive verbatim, so formatting still works under the pseudo-locale. Pure and
//!   deterministic; [`is_pseudo`] recognises the result.
//! * **[`Localiser`]** — the runtime lookup: current locale, then the source locale, then a
//!   visible `⟦key⟧` marker (a missing string is shown, never an empty label).
//! * **[`LocalisedTexts`]** — the game-UI binding: labels bound to a key through a
//!   `Signal<String>`; switching language re-sets only the signals whose text changed, so a
//!   language switch re-shapes what changed and nothing else (§21.7, D-5).
//! * **CSV** ([`to_csv`], [`parse_csv`]) — the translator hand-off format: `key` then one
//!   column per locale, RFC 4180 quoting.

use std::collections::{BTreeMap, BTreeSet};

use crate::state::{Runtime, Signal};

/// The pseudo-locale's tag (Microsoft's `qps-ploc`, which translation tools recognise).
pub const PSEUDO_LOCALE: &str = "qps-ploc";

/// Opening and closing marks of a pseudo-localised string.
const PSEUDO_OPEN: char = '[';
const PSEUDO_CLOSE: char = ']';
/// The expansion filler (Latin-1, in every bundled font).
const PSEUDO_PAD: char = '\u{b7}';

/// One piece of a template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    /// `{{` or `}}`: one literal brace.
    Brace(char),
    /// `{name}`.
    Placeholder(&'a str),
}

/// Split a template into pieces. A `{` with no matching `}` (or with a character that is
/// not an identifier character inside) is literal text, so any string parses.
pub fn pieces(s: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    for_each_piece(s, |p| out.push(p));
    out
}

/// [`pieces`] handed to `f` one by one, with no list built (a format into a reused buffer
/// allocates nothing).
pub fn for_each_piece<'a>(s: &'a str, mut f: impl FnMut(Piece<'a>)) {
    let b = s.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if (c == b'{' || c == b'}') && b.get(i + 1) == Some(&c) {
            if start < i {
                f(Piece::Text(&s[start..i]));
            }
            f(Piece::Brace(c as char));
            i += 2;
            start = i;
            continue;
        }
        if c == b'{' {
            let rest = &s[i + 1..];
            if let Some(end) = rest.find('}') {
                let name = &rest[..end];
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    if start < i {
                        f(Piece::Text(&s[start..i]));
                    }
                    f(Piece::Placeholder(name));
                    i += end + 2;
                    start = i;
                    continue;
                }
            }
        }
        i += 1;
    }
    if start < b.len() {
        f(Piece::Text(&s[start..]));
    }
}

/// The placeholder names of a template, in order of first appearance.
pub fn placeholders(s: &str) -> Vec<&str> {
    let mut seen = BTreeSet::new();
    pieces(s)
        .into_iter()
        .filter_map(|p| match p {
            Piece::Placeholder(n) if seen.insert(n) => Some(n),
            _ => None,
        })
        .collect()
}

/// Fill a template's placeholders from `args` (`(name, value)`); an unknown placeholder is
/// left as `{name}` so the gap is visible.
pub fn format(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    for p in pieces(template) {
        match p {
            Piece::Text(t) => out.push_str(t),
            Piece::Brace(c) => out.push(c),
            Piece::Placeholder(n) => match args.iter().find(|(k, _)| *k == n) {
                Some((_, v)) => out.push_str(v),
                None => {
                    out.push('{');
                    out.push_str(n);
                    out.push('}');
                }
            },
        }
    }
    out
}

/// What a translation does to its source's placeholders: `(missing, extra)`. Both empty:
/// the translation formats with the same arguments.
pub fn placeholder_mismatch(source: &str, translation: &str) -> (Vec<String>, Vec<String>) {
    let s: BTreeSet<&str> = placeholders(source).into_iter().collect();
    let t: BTreeSet<&str> = placeholders(translation).into_iter().collect();
    (
        s.difference(&t).map(|x| (*x).to_string()).collect(),
        t.difference(&s).map(|x| (*x).to_string()).collect(),
    )
}

/// The accented look-alike of an ASCII letter (Latin-1 and Latin Extended-A only: every
/// bundled font draws them).
fn accent(c: char) -> char {
    match c {
        'a' => '\u{e1}',
        'c' => '\u{e7}',
        'd' => '\u{10f}',
        'e' => '\u{e9}',
        'g' => '\u{11d}',
        'h' => '\u{125}',
        'i' => '\u{ed}',
        'j' => '\u{135}',
        'k' => '\u{137}',
        'l' => '\u{13a}',
        'n' => '\u{f1}',
        'o' => '\u{f6}',
        'r' => '\u{155}',
        's' => '\u{161}',
        't' => '\u{163}',
        'u' => '\u{fc}',
        'w' => '\u{175}',
        'y' => '\u{fd}',
        'z' => '\u{17e}',
        'A' => '\u{c1}',
        'C' => '\u{c7}',
        'D' => '\u{10e}',
        'E' => '\u{c9}',
        'G' => '\u{11c}',
        'H' => '\u{124}',
        'I' => '\u{cd}',
        'J' => '\u{134}',
        'K' => '\u{136}',
        'L' => '\u{139}',
        'N' => '\u{d1}',
        'O' => '\u{d6}',
        'R' => '\u{154}',
        'S' => '\u{160}',
        'T' => '\u{162}',
        'U' => '\u{dc}',
        'W' => '\u{174}',
        'Y' => '\u{dd}',
        'Z' => '\u{17d}',
        other => other,
    }
}

/// How many filler characters a text of `n` visible characters gains (about 40 %, at least
/// two: short strings grow the most in real translations).
pub fn pseudo_expansion(n: usize) -> usize {
    (n * 2).div_ceil(5).max(2)
}

/// Pseudo-localise a template (see the module docs). Placeholders and brace escapes are
/// kept verbatim; the result is `[` + accented text + ` ` + filler + `]`.
pub fn pseudo_localise(s: &str) -> String {
    let mut body = String::with_capacity(s.len() * 2 + 8);
    let mut visible = 0usize;
    for p in pieces(s) {
        match p {
            Piece::Text(t) => {
                for c in t.chars() {
                    body.push(accent(c));
                    visible += 1;
                }
            }
            Piece::Brace(c) => {
                body.push(c);
                body.push(c);
                visible += 1;
            }
            Piece::Placeholder(n) => {
                body.push('{');
                body.push_str(n);
                body.push('}');
                visible += 4;
            }
        }
    }
    let mut out = String::with_capacity(body.len() + 16);
    out.push(PSEUDO_OPEN);
    out.push_str(&body);
    out.push(' ');
    for _ in 0..pseudo_expansion(visible) {
        out.push(PSEUDO_PAD);
    }
    out.push(PSEUDO_CLOSE);
    out
}

/// Was `s` produced by [`pseudo_localise`] (after formatting)? Used by the pseudo-locale
/// audit: a visible string that is not pseudo was not looked up — it is hard-coded.
pub fn is_pseudo(s: &str) -> bool {
    s.starts_with(PSEUDO_OPEN)
        && s.ends_with(PSEUDO_CLOSE)
        && s.trim_end_matches(PSEUDO_CLOSE).ends_with(PSEUDO_PAD)
}

/// A missing string, shown as `⟦key⟧`.
pub fn missing_marker(key: &str) -> String {
    format!("\u{27e6}{key}\u{27e7}")
}

/// The runtime string lookup (see the module docs). `tables[locale][key]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Localiser {
    tables: BTreeMap<String, BTreeMap<String, String>>,
    source: String,
    current: String,
}

impl Localiser {
    /// A localiser whose source (fallback) locale is `source`.
    pub fn new(source: &str) -> Self {
        Self {
            tables: BTreeMap::new(),
            source: source.to_string(),
            current: source.to_string(),
        }
    }
    /// Add or replace one string.
    pub fn insert(&mut self, locale: &str, key: &str, text: &str) {
        self.tables
            .entry(locale.to_string())
            .or_default()
            .insert(key.to_string(), text.to_string());
    }
    /// Builder form of [`Localiser::insert`] over many `(key, text)` pairs.
    pub fn with(mut self, locale: &str, strings: &[(&str, &str)]) -> Self {
        for (k, t) in strings {
            self.insert(locale, k, t);
        }
        self
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn current(&self) -> &str {
        &self.current
    }
    /// The locales with strings, plus the pseudo-locale (always available).
    pub fn locales(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tables.keys().cloned().collect();
        if !v.iter().any(|l| l == PSEUDO_LOCALE) {
            v.push(PSEUDO_LOCALE.to_string());
        }
        v
    }
    /// Switch language. Returns false (and changes nothing) for a locale with no strings
    /// that is not the pseudo-locale.
    pub fn set_current(&mut self, locale: &str) -> bool {
        if locale == PSEUDO_LOCALE || self.tables.contains_key(locale) {
            self.current = locale.to_string();
            true
        } else {
            false
        }
    }
    /// The raw template of `key` in `locale`, if it has one.
    pub fn raw(&self, locale: &str, key: &str) -> Option<&str> {
        self.tables
            .get(locale)
            .and_then(|t| t.get(key))
            .map(String::as_str)
    }
    /// The template for `key`: the current locale's, then the source locale's; under the
    /// pseudo-locale, the source text pseudo-localised; `⟦key⟧` when there is none.
    pub fn template(&self, key: &str) -> String {
        if self.current == PSEUDO_LOCALE {
            return match self.raw(&self.source, key) {
                Some(t) => pseudo_localise(t),
                None => missing_marker(key),
            };
        }
        self.raw(&self.current, key)
            .or_else(|| self.raw(&self.source, key))
            .map_or_else(|| missing_marker(key), str::to_string)
    }
    /// The text for `key`, formatted with `args`.
    pub fn get(&self, key: &str, args: &[(&str, &str)]) -> String {
        format(&self.template(key), args)
    }
}

/// One bound label: its key, its format arguments and its signal.
type BoundText = (String, Vec<(String, String)>, Signal<String>);

/// Labels bound to string keys (see the module docs).
#[derive(Default)]
pub struct LocalisedTexts {
    bound: Vec<BoundText>,
}

impl LocalisedTexts {
    pub fn new() -> Self {
        Self::default()
    }
    /// A signal holding `key`'s text now; [`LocalisedTexts::apply`] keeps it current.
    pub fn bind(&mut self, rt: &mut Runtime, loc: &Localiser, key: &str) -> Signal<String> {
        self.bind_args(rt, loc, key, &[])
    }
    /// As [`LocalisedTexts::bind`], formatted with `args`.
    pub fn bind_args(
        &mut self,
        rt: &mut Runtime,
        loc: &Localiser,
        key: &str,
        args: &[(&str, &str)],
    ) -> Signal<String> {
        let s = rt.signal(loc.get(key, args));
        self.bound.push((
            key.to_string(),
            args.iter()
                .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
                .collect(),
            s,
        ));
        s
    }
    /// Change one bound signal's arguments (a HUD score) and re-format it.
    pub fn set_args(
        &mut self,
        rt: &mut Runtime,
        loc: &Localiser,
        s: Signal<String>,
        args: &[(&str, &str)],
    ) {
        if let Some((key, a, sig)) = self.bound.iter_mut().find(|(_, _, x)| *x == s) {
            *a = args
                .iter()
                .map(|(x, y)| ((*x).to_string(), (*y).to_string()))
                .collect();
            let pairs: Vec<(&str, &str)> =
                a.iter().map(|(x, y)| (x.as_str(), y.as_str())).collect();
            sig.set(rt, loc.get(key, &pairs));
        }
    }
    /// Re-set every bound signal from `loc` (after a language switch). Signals whose text
    /// did not change are not written (an equal `set` is a no-op, §21.5). Returns how many
    /// changed.
    pub fn apply(&self, rt: &mut Runtime, loc: &Localiser) -> usize {
        let mut changed = 0;
        for (key, args, s) in &self.bound {
            let pairs: Vec<(&str, &str)> =
                args.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            let text = loc.get(key, &pairs);
            if s.get(rt) != text {
                s.set(rt, text);
                changed += 1;
            }
        }
        changed
    }
    pub fn len(&self) -> usize {
        self.bound.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }
    /// The keys bound (the pseudo-locale audit walks them).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.bound.iter().map(|(k, _, _)| k.as_str())
    }
}

// ---- CSV ----------------------------------------------------------------------------------

fn csv_field(out: &mut String, f: &str) {
    if f.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&f.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(f);
    }
}

/// Rows as CSV: the header `key,<locale>...`, then one row per key. `rows[i] = (key,
/// texts)` with `texts` aligned with `locales` (`None`: empty cell).
pub fn to_csv(locales: &[&str], rows: &[(String, Vec<Option<String>>)]) -> String {
    let mut out = String::new();
    out.push_str("key");
    for l in locales {
        out.push(',');
        csv_field(&mut out, l);
    }
    out.push('\n');
    for (k, texts) in rows {
        csv_field(&mut out, k);
        for i in 0..locales.len() {
            out.push(',');
            if let Some(Some(t)) = texts.get(i) {
                csv_field(&mut out, t);
            }
        }
        out.push('\n');
    }
    out
}

/// Parse CSV into records (RFC 4180: quoted fields, doubled quotes, newlines inside
/// quotes; CRLF or LF). An unterminated quote is an error naming the line it opened on.
pub fn parse_csv_records(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut rec: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut opened_at = 0usize;
    let mut line = 1usize;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => {
                quoted = true;
                opened_at = line;
            }
            ',' => rec.push(std::mem::take(&mut field)),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                line += 1;
                rec.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut rec));
                any = false;
            }
            _ => field.push(c),
        }
    }
    if quoted {
        return Err(format!("line {opened_at}: a quoted field is not closed"));
    }
    if any {
        rec.push(field);
        records.push(rec);
    }
    Ok(records)
}

/// A translator's CSV: `(locales, rows)` where each row is `(key, texts aligned with
/// locales)`; an empty cell is `None`. The header must start with `key`.
pub type CsvTable = (Vec<String>, Vec<(String, Vec<Option<String>>)>);

/// Parse a translator's CSV (see [`CsvTable`]).
pub fn parse_csv(text: &str) -> Result<CsvTable, String> {
    let mut recs = parse_csv_records(text)?.into_iter();
    let header = recs.next().ok_or("the file is empty")?;
    if header.first().map(|h| h.trim()) != Some("key") {
        return Err("the first column must be headed \"key\"".into());
    }
    let locales: Vec<String> = header[1..].iter().map(|l| l.trim().to_string()).collect();
    let mut rows = Vec::new();
    for (i, r) in recs.enumerate() {
        if r.iter().all(|f| f.is_empty()) {
            continue;
        }
        if r.len() > locales.len() + 1 {
            return Err(format!(
                "row {}: {} cells, the header has {}",
                i + 2,
                r.len(),
                locales.len() + 1
            ));
        }
        let key = r[0].trim().to_string();
        let texts = (0..locales.len())
            .map(|j| r.get(j + 1).filter(|t| !t.is_empty()).cloned())
            .collect();
        rows.push((key, texts));
    }
    Ok((locales, rows))
}

// ---- UI strings: every UI string is a localisation key (M2-31, ADR 0046) -----------------

/// The UI language of this thread (the UI thread: a window's widgets are built and drawn
/// there). `None` is the source language: [`tr`] returns its key unchanged, allocating
/// nothing.
#[derive(Default)]
struct UiLocale {
    /// Shared, so a lookup under a non-source locale clones a pointer, not the tag.
    locale: Option<std::rc::Rc<str>>,
    /// The active locale's translations (`key -> text`).
    table: BTreeMap<String, String>,
    /// Looked-up texts per locale, leaked once each so [`tr`] can hand out `&'static str`
    /// (bounded: keys x locales used in this run; switching back reuses them).
    interned: std::collections::HashMap<
        std::rc::Rc<str>,
        std::collections::HashMap<&'static str, &'static str>,
    >,
}

thread_local! {
    static UI_LOCALE: std::cell::RefCell<UiLocale> = std::cell::RefCell::new(UiLocale::default());
}

/// Set this thread's UI language: `locale` with its translations (`key -> text`; a key
/// missing from `table` shows its source text). [`PSEUDO_LOCALE`] needs no table. The
/// editor builds its windows after this, so the language applies from the next start of
/// a window (Ch.21 §21.21 "Localisation").
pub fn set_ui_locale(locale: &str, table: BTreeMap<String, String>) {
    UI_LOCALE.with(|l| {
        let mut l = l.borrow_mut();
        l.locale = Some(std::rc::Rc::from(locale));
        l.table = table;
        // A new table for this locale: forget what was looked up under the old one.
        l.interned.remove(locale);
    });
}

/// Back to the source language on this thread.
pub fn clear_ui_locale() {
    UI_LOCALE.with(|l| {
        let mut l = l.borrow_mut();
        l.locale = None;
        l.table = BTreeMap::new();
    });
}

/// This thread's UI language (`None`: the source language).
pub fn ui_locale() -> Option<String> {
    UI_LOCALE.with(|l| l.borrow().locale.as_deref().map(str::to_string))
}

fn translate(l: &UiLocale, locale: &str, key: &str) -> String {
    if locale == PSEUDO_LOCALE {
        // Text that was already looked up (a composed label passed through `tr_str` again
        // where it is shown) stays as it is.
        if is_pseudo(key) {
            key.to_string()
        } else {
            pseudo_localise(key)
        }
    } else {
        l.table.get(key).cloned().unwrap_or_else(|| key.to_string())
    }
}

/// The UI text for the string key `key` (its source-language text is the key; the
/// [`tr!`](crate::tr) macro is the usual spelling). Source language: `key` itself. Another
/// locale: looked up once, then a hash lookup with no allocation.
pub fn tr(key: &'static str) -> &'static str {
    UI_LOCALE.with(|l| {
        let mut l = l.borrow_mut();
        let Some(locale) = l.locale.clone() else {
            return key;
        };
        if let Some(t) = l.interned.get(&*locale).and_then(|m| m.get(key)) {
            return *t;
        }
        let text: &'static str = Box::leak(translate(&l, &locale, key).into_boxed_str());
        l.interned.entry(locale).or_default().insert(key, text);
        text
    })
}

/// The UI text for a key known only at run time (a reflected field's label and tooltip, a
/// plugin's panel title): the same lookup as [`tr`], not interned.
pub fn tr_str(key: &str) -> std::borrow::Cow<'_, str> {
    UI_LOCALE.with(|l| {
        let l = l.borrow();
        match &l.locale {
            None => std::borrow::Cow::Borrowed(key),
            Some(locale) => std::borrow::Cow::Owned(translate(&l, locale, key)),
        }
    })
}

/// A formatted UI string: `template` (a string key with `{name}` placeholders) looked up,
/// then filled from `args`, each written straight into the result (no per-argument string).
/// The [`trf!`](crate::trf) macro is the usual spelling.
pub fn tr_format(template: &'static str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = String::with_capacity(tr(template).len() + 16);
    tr_format_into(&mut out, template, args);
    out
}

/// [`tr_format`] appended to `out`, a buffer reused across frames: nothing is allocated once
/// it has grown to the longest text written into it (the arguments are written straight in,
/// so they must not allocate either: see [`TrStr`]). The [`trf_into!`](crate::trf_into)
/// macro is the usual spelling.
pub fn tr_format_into(
    out: &mut String,
    template: &'static str,
    args: &[(&str, &dyn std::fmt::Display)],
) {
    use std::fmt::Write as _;
    let t = tr(template);
    for_each_piece(t, |p| match p {
        Piece::Text(s) => out.push_str(s),
        Piece::Brace(c) => out.push(c),
        Piece::Placeholder(n) => match args.iter().find(|(k, _)| *k == n) {
            // Writing into a String cannot fail.
            Some((_, v)) => {
                let _ = write!(out, "{v}");
            }
            None => {
                out.push('{');
                out.push_str(n);
                out.push('}');
            }
        },
    });
}

/// The UI text of a key known only at run time as a [`Display`](std::fmt::Display), written
/// straight from the locale's table: [`tr_str`] without its `String` under a translated
/// locale (the pseudo-locale, a test locale, still builds its text).
#[derive(Clone, Copy, Debug)]
pub struct TrStr<'a>(pub &'a str);

impl std::fmt::Display for TrStr<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        UI_LOCALE.with(|l| {
            let l = l.borrow();
            match l.locale.as_deref() {
                None => f.write_str(self.0),
                Some(PSEUDO_LOCALE) => f.write_str(&translate(&l, PSEUDO_LOCALE, self.0)),
                Some(_) => f.write_str(l.table.get(self.0).map_or(self.0, String::as_str)),
            }
        })
    }
}

/// A UI string: `tr!("Save")` is the text of the string key `"Save"` in this thread's UI
/// language (M2-31: every editor string is a localisation key; the pseudo-locale audit
/// finds one that is not). Returns `&'static str`.
#[macro_export]
macro_rules! tr {
    ($key:literal) => {
        $crate::l10n::tr($key)
    };
}

/// A string key that is looked up where it is shown, not where it is written: an action's
/// title, a panel's title, a tip — data a registry carries until a menu, the palette or a
/// tab bar shows it through [`tr_str`]. Expands to the key itself (`&'static str`); it
/// names the literal as a key for the translator's extraction and for the source audit
/// (`test_ui_strings_are_keys`), like gettext's `N_()`.
#[macro_export]
macro_rules! tr_key {
    ($key:literal) => {
        $key
    };
}

/// A formatted UI string: `trf!("{n} revisions", n = count)` or `trf!("{n} revisions", n)`.
/// The template is the key, so a translator reorders or rewords around the placeholders;
/// `{{`/`}}` are literal braces. Returns `String`.
#[macro_export]
macro_rules! trf {
    ($key:literal $(, $name:ident $(= $value:expr)?)* $(,)?) => {
        $crate::l10n::tr_format(
            $key,
            &[$((stringify!($name), &$crate::trf!(@value $name $($value)?) as &dyn ::std::fmt::Display)),*],
        )
    };
    (@value $name:ident $value:expr) => {
        $value
    };
    (@value $name:ident) => {
        $name
    };
}

/// [`trf!`](crate::trf) appended to a reused `&mut String` (see [`tr_format_into`]):
/// `trf_into!(&mut buf, "{n} revisions", n)`. The template is the key, as in `trf!`.
#[macro_export]
macro_rules! trf_into {
    ($out:expr, $key:literal $(, $name:ident $(= $value:expr)?)* $(,)?) => {
        $crate::l10n::tr_format_into(
            $out,
            $key,
            &[$((stringify!($name), &$crate::trf!(@value $name $($value)?) as &dyn ::std::fmt::Display)),*],
        )
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn trf_into_and_tr_str_display_match_their_allocating_forms() {
        let check = || {
            let (n, what) = (3, super::TrStr("Save"));
            let mut buf = String::from("stale");
            buf.clear();
            crate::trf_into!(&mut buf, "{n} revisions of {what}", n, what);
            assert_eq!(
                buf,
                crate::trf!("{n} revisions of {what}", n, what = super::tr_str("Save"))
            );
            assert_eq!(super::TrStr("Save").to_string(), super::tr_str("Save"));
        };
        super::clear_ui_locale();
        check();
        let mut de = BTreeMap::new();
        de.insert("Save".to_string(), "Speichern".to_string());
        super::set_ui_locale("de", de);
        check();
        assert_eq!(super::TrStr("Save").to_string(), "Speichern");
        super::set_ui_locale(super::PSEUDO_LOCALE, BTreeMap::new());
        check();
        super::clear_ui_locale();
    }

    #[test]
    fn tr_is_the_key_in_the_source_language_and_pseudo_under_the_pseudo_locale() {
        super::clear_ui_locale();
        let key: &'static str = "Save";
        assert!(
            std::ptr::eq(super::tr(key), key),
            "no allocation in the source language"
        );
        super::set_ui_locale(super::PSEUDO_LOCALE, BTreeMap::new());
        let p = crate::tr!("Save");
        assert!(super::is_pseudo(p), "{p}");
        assert!(
            std::ptr::eq(p, crate::tr!("Save")),
            "interned once per locale"
        );
        let n = 3;
        let f = crate::trf!("{n} revisions of {what}", n, what = "x");
        assert!(
            super::is_pseudo(&f) && f.contains("3") && f.contains("x"),
            "{f}"
        );
        let mut de = BTreeMap::new();
        de.insert("Save".to_string(), "Speichern".to_string());
        super::set_ui_locale("de", de);
        assert_eq!(crate::tr!("Save"), "Speichern");
        assert_eq!(
            crate::tr!("Cancel"),
            "Cancel",
            "a missing key shows its source text"
        );
        assert_eq!(super::tr_str("Save"), "Speichern");
        super::clear_ui_locale();
        assert_eq!(super::tr_str("Save"), "Save");
    }

    #[test]
    fn text_already_looked_up_is_not_pseudo_localised_twice() {
        super::set_ui_locale(super::PSEUDO_LOCALE, BTreeMap::new());
        let once = crate::trf!("Show {panel}", panel = super::tr_str("Assets"));
        assert!(super::is_pseudo(&once), "{once}");
        assert_eq!(
            super::tr_str(&once),
            once,
            "a composed label shown through tr_str"
        );
        let filled = crate::trf!("{a} of {b}", a = 3, b = "x");
        assert!(
            filled.starts_with("[3 ") && filled.contains(" x "),
            "{filled}"
        );
        super::clear_ui_locale();
        assert_eq!(crate::trf!("{a} of {b}", a = 3, b = "x"), "3 of x");
        assert_eq!(crate::trf!("{missing} {{x}}"), "{missing} {x}");
    }

    use super::*;

    #[test]
    fn placeholders_parse_and_format() {
        assert_eq!(
            placeholders("Hi {name}, {n} new {{not}} {n}"),
            vec!["name", "n"]
        );
        assert_eq!(format("Score: {score}", &[("score", "42")]), "Score: 42");
        assert_eq!(format("{a} {{b}} {c", &[("a", "1")]), "1 {b} {c");
        assert_eq!(format("{missing}", &[]), "{missing}");
        let (m, e) = placeholder_mismatch("{a} and {b}", "{a} et {c}");
        assert_eq!((m, e), (vec!["b".to_string()], vec!["c".to_string()]));
    }

    #[test]
    fn pseudo_localisation_accents_expands_brackets_and_keeps_placeholders() {
        let p = pseudo_localise("Play {count} games");
        assert!(p.starts_with('['), "{p}");
        assert!(p.contains("{count}"), "{p}");
        assert!(!p.contains("Play"), "letters are accented: {p}");
        assert!(is_pseudo(&p));
        assert!(!is_pseudo("Play"));
        assert!(!is_pseudo("[Play]"));
        // Formatting still works under the pseudo-locale.
        assert!(format(&p, &[("count", "3")]).contains(" 3 "));
        // ~40 % longer (in characters), and never shorter than + 2.
        let n = "Settings".chars().count();
        assert!(pseudo_localise("Settings").chars().count() >= n + pseudo_expansion(n) + 3);
        assert_eq!(pseudo_expansion(1), 2);
        assert_eq!(pseudo_expansion(10), 4);
        // Deterministic.
        assert_eq!(pseudo_localise("Quit"), pseudo_localise("Quit"));
    }

    #[test]
    fn localiser_falls_back_and_marks_missing() {
        let mut l = Localiser::new("en")
            .with("en", &[("menu.play", "Play"), ("hud.score", "Score: {n}")])
            .with("fr", &[("menu.play", "Jouer")]);
        assert_eq!(l.get("menu.play", &[]), "Play");
        assert!(l.set_current("fr"));
        assert_eq!(l.get("menu.play", &[]), "Jouer");
        assert_eq!(
            l.get("hud.score", &[("n", "7")]),
            "Score: 7",
            "falls back to en"
        );
        assert_eq!(l.get("nope", &[]), "\u{27e6}nope\u{27e7}");
        assert!(!l.set_current("de"));
        assert!(l.set_current(PSEUDO_LOCALE));
        assert!(is_pseudo(&l.get("hud.score", &[("n", "7")])));
        assert!(l.locales().contains(&PSEUDO_LOCALE.to_string()));
    }

    #[test]
    fn localised_texts_rewrite_only_what_changed() {
        let mut rt = Runtime::new();
        let mut l = Localiser::new("en")
            .with("en", &[("a", "Play"), ("b", "OK")])
            .with("fr", &[("a", "Jouer"), ("b", "OK")]);
        let mut t = LocalisedTexts::new();
        let a = t.bind(&mut rt, &l, "a");
        let b = t.bind(&mut rt, &l, "b");
        assert!(l.set_current("fr"));
        assert_eq!(t.apply(&mut rt, &l), 1, "only the label whose text changed");
        assert_eq!(a.get(&rt), "Jouer");
        assert_eq!(b.get(&rt), "OK");
    }

    #[test]
    fn csv_round_trips_quotes_commas_and_newlines() {
        let rows = vec![
            (
                "menu.play".to_string(),
                vec![Some("Play".to_string()), Some("Jouer".to_string())],
            ),
            (
                "hud.msg".to_string(),
                vec![Some("Say \"hi\", then\nleave".to_string()), None],
            ),
        ];
        let csv = to_csv(&["en", "fr"], &rows);
        let (locales, back) = parse_csv(&csv).expect("parses");
        assert_eq!(locales, vec!["en", "fr"]);
        assert_eq!(back, rows);
        assert!(parse_csv("key,en\n\"open,x\n").is_err());
        assert!(parse_csv("id,en\n").is_err());
        assert_eq!(
            parse_csv("key,en\r\na,b\r\n").expect("crlf").1,
            vec![("a".to_string(), vec![Some("b".to_string())])]
        );
    }
}
