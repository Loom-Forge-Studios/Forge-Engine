//! Every editor UI string is a localisation key (DoD M2-31, Ch.21 §21.21 "Localisation",
//! ADR 0046 §6) — the **source** half of the guard (gate `C-ui-strings-source`).
//!
//! `test_pseudo_locale_no_hardcoded_strings` (test_editor_panels_audit.rs) reads what the
//! panels show; it only sees the states its passes reach. A refusal, a warning, a history
//! label or a status line that only appears after a failed import would slip past it. This
//! test reads the code instead: every string literal in the editor's UI code (the
//! first-party panel plugins, the shell's UI modules, forge-ui's widgets) that reads as
//! words is the key of a `tr!` / `trf!` / `trf_into!` / `tr_str` / `tr_format` lookup, or a `tr_key!` (a key
//! looked up where it is shown), or carries an `// l10n: <reason>` note on its line or the
//! line above (data: a manifest, a number format, an identifier-like default name). A block
//! whose strings are all diagnostic detail (an error's `Display`, next to its stable code)
//! is marked `// l10n-block: <reason>` and skipped whole.
//!
//! What counts as words: two alphabetic runs separated by a space with one of at least three
//! letters ("Rename refused", "{n} assets"), or one capitalised word ("Delete", "Import…").
//! Keys, ids and paths (`forge.assets`, `row:{path}`, `status`) are not words.
//!
//! Control: a snippet with one hard-coded label is flagged; the same snippet through `tr!`,
//! with a note, or in a `#[cfg(test)]` module is not.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Punct(char),
    Open(char),
    Close(char),
    Str {
        text: String,
        line: usize,
        pos: usize,
    },
}

/// A lexed file: its tokens (each with the line it starts on) and its `// l10n` notes.
struct Lexed {
    toks: Vec<(Tok, usize)>,
    /// Lines with an `// l10n: ...` note.
    notes: Vec<usize>,
    /// Lines with an `// l10n-block: ...` note.
    block_notes: Vec<usize>,
}

fn lex(src: &str) -> Lexed {
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line = 1;
    let mut toks = Vec::new();
    let mut notes = Vec::new();
    let mut block_notes = Vec::new();
    let at = |i: usize| c.get(i).copied().unwrap_or('\0');
    while i < c.len() {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '/' && at(i + 1) == '/' {
            let start = i;
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            let text: String = c[start..i].iter().collect();
            if text.contains("l10n-block:") {
                block_notes.push(line);
            } else if text.contains("l10n:") {
                notes.push(line);
            }
            continue;
        }
        if ch == '/' && at(i + 1) == '*' {
            let mut depth = 0;
            while i < c.len() {
                if c[i] == '/' && at(i + 1) == '*' {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && at(i + 1) == '/' {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if c[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
            continue;
        }
        // Raw strings (r"..", r#".."#, br".."), byte strings (b"..").
        let raw_start = if ch == 'r' && (at(i + 1) == '"' || at(i + 1) == '#') {
            Some(i + 1)
        } else if ch == 'b' && at(i + 1) == 'r' && (at(i + 2) == '"' || at(i + 2) == '#') {
            Some(i + 2)
        } else {
            None
        };
        if let Some(mut j) = raw_start {
            let mut hashes = 0;
            while at(j) == '#' {
                hashes += 1;
                j += 1;
            }
            if at(j) == '"' {
                let start_line = line;
                let pos = i;
                j += 1;
                let body_start = j;
                loop {
                    if j >= c.len() {
                        break;
                    }
                    if c[j] == '"' && (0..hashes).all(|h| at(j + 1 + h) == '#') {
                        break;
                    }
                    if c[j] == '\n' {
                        line += 1;
                    }
                    j += 1;
                }
                let text: String = c[body_start..j.min(c.len())].iter().collect();
                if ch == 'r' {
                    toks.push((
                        Tok::Str {
                            text,
                            line: start_line,
                            pos,
                        },
                        start_line,
                    ));
                }
                i = j + 1 + hashes;
                continue;
            }
        }
        if ch == 'b' && at(i + 1) == '"' {
            i += 2;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' {
                    i += 1;
                }
                if at(i) == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if ch == 'b' && at(i + 1) == '\'' {
            i += 2;
            while i < c.len() && c[i] != '\'' {
                if c[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            toks.push((Tok::Ident(c[start..i].iter().collect()), line));
            continue;
        }
        if ch.is_ascii_digit() {
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            continue;
        }
        if ch == '"' {
            let start_line = line;
            let pos = i;
            let mut text = String::new();
            i += 1;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' {
                    match at(i + 1) {
                        '\n' => {
                            // Line continuation: the newline and the next line's indent go.
                            line += 1;
                            i += 2;
                            while i < c.len() && c[i].is_whitespace() {
                                if c[i] == '\n' {
                                    line += 1;
                                }
                                i += 1;
                            }
                            continue;
                        }
                        'u' => {
                            // \u{..}: a symbol, not a letter the check reads.
                            while i < c.len() && c[i] != '}' {
                                i += 1;
                            }
                            text.push(' ');
                            i += 1;
                            continue;
                        }
                        'n' | 't' | 'r' => {
                            text.push(' ');
                            i += 2;
                            continue;
                        }
                        other => {
                            text.push(other);
                            i += 2;
                            continue;
                        }
                    }
                }
                if c[i] == '\n' {
                    line += 1;
                }
                text.push(c[i]);
                i += 1;
            }
            i += 1;
            toks.push((
                Tok::Str {
                    text,
                    line: start_line,
                    pos,
                },
                start_line,
            ));
            continue;
        }
        if ch == '\'' {
            // A char literal ('x', '\n', '\u{..}') or a lifetime / label ('a).
            if at(i + 1) == '\\' {
                i += 2;
                while i < c.len() && c[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if at(i + 2) == '\'' {
                i += 3;
            } else {
                i += 1;
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                    i += 1;
                }
            }
            continue;
        }
        match ch {
            '(' | '[' | '{' => toks.push((Tok::Open(ch), line)),
            ')' | ']' | '}' => toks.push((Tok::Close(ch), line)),
            _ => toks.push((Tok::Punct(ch), line)),
        }
        i += 1;
    }
    Lexed {
        toks,
        notes,
        block_notes,
    }
}

/// Does `s` read as words a translator would translate (see the module docs)?
fn is_words(s: &str) -> bool {
    // Placeholders are filled from data; `{{`/`}}` are braces.
    let mut t = String::new();
    let mut depth = 0;
    for ch in s.chars() {
        match ch {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => t.push(ch),
            _ => {}
        }
    }
    let runs: Vec<&str> = t
        .split(|ch: char| !ch.is_alphabetic() && ch != '\'')
        .filter(|w| !w.is_empty())
        .collect();
    let spaced = t.trim().contains(' ');
    if spaced && runs.len() >= 2 && runs.iter().any(|w| w.chars().count() >= 3) {
        // Identifier-like pieces (`a.b`, `a_b`, `a::b`) with a space are code, not words.
        let wordy = t
            .split_whitespace()
            .filter(|w| {
                let w = w.trim_matches(|ch: char| !ch.is_alphanumeric());
                !w.is_empty()
                    && !w.contains('_')
                    && !w.contains("::")
                    && !(w.contains('.') && w.chars().any(char::is_alphabetic))
            })
            .count();
        if wordy >= 2 {
            return true;
        }
    }
    // A template with one word beside its placeholders: "{n} untranslated", "{} (muted)".
    let placeholders = s.contains('{') && t.len() < s.len();
    if placeholders
        && t.contains(char::is_whitespace)
        && t.split_whitespace().any(|w| {
            // "untranslated", "(muted)", "command(s)"; not `a.b`, `a_b`, `a::b`.
            !w.contains('_')
                && !w.contains("::")
                && !(w.contains('.') && w.trim_matches('.').contains('.'))
                && w.split(|ch: char| !ch.is_alphabetic())
                    .any(|run| run.chars().count() >= 3)
        })
    {
        return true;
    }
    // One capitalised word: "Delete", "Import…", "Rename:".
    let w = t.trim().trim_end_matches(['.', ':', '!', '?', '\u{2026}']);
    let mut ch = w.chars();
    matches!(ch.next(), Some(f) if f.is_uppercase())
        && w.chars().count() >= 3
        && ch.all(|x| x.is_lowercase())
}

/// The literals of `src` that are words and not looked up: `(line, text)`.
fn findings(src: &str) -> Vec<(usize, usize, String)> {
    findings_where(src, None)
}

/// As [`findings`], limited to the bodies of the functions named `only` when given (a model
/// module's presentational methods: what it returns is shown as it is).
fn findings_where(src: &str, only: Option<&[&str]>) -> Vec<(usize, usize, String)> {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(
            src.chars()
                .enumerate()
                .filter(|(_, ch)| *ch == '\n')
                .map(|(k, _)| k + 1),
        )
        .collect();
    let lx = lex(src);
    let t = &lx.toks;
    let mut out = Vec::new();
    let mut exempt = vec![false; t.len()];
    // Matching close for each open.
    let mut close_of = vec![usize::MAX; t.len()];
    let mut stack = Vec::new();
    for (k, (tok, _)) in t.iter().enumerate() {
        match tok {
            Tok::Open(_) => stack.push(k),
            Tok::Close(_) => {
                if let Some(o) = stack.pop() {
                    close_of[o] = k;
                }
            }
            _ => {}
        }
    }
    let mut k = 0;
    while k < t.len() {
        // Attributes: `#[..]`, `#![..]`; `#[cfg(test)]` also skips the item it marks.
        if t[k].0 == Tok::Punct('#') {
            let mut o = k + 1;
            if t.get(o).map(|x| &x.0) == Some(&Tok::Punct('!')) {
                o += 1;
            }
            if t.get(o).map(|x| &x.0) == Some(&Tok::Open('[')) && close_of[o] != usize::MAX {
                let end = close_of[o];
                let idents: Vec<&str> = t[o..end]
                    .iter()
                    .filter_map(|(x, _)| match x {
                        Tok::Ident(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .collect();
                for e in exempt.iter_mut().take(end + 1).skip(k) {
                    *e = true;
                }
                let test_only = idents.first() == Some(&"cfg")
                    && idents.contains(&"test")
                    && !idents.contains(&"not");
                k = end + 1;
                if test_only {
                    // Skip the item: through `;` or its first brace group.
                    while k < t.len() {
                        match &t[k].0 {
                            Tok::Punct(';') => {
                                exempt[k] = true;
                                k += 1;
                                break;
                            }
                            Tok::Open('{') if close_of[k] != usize::MAX => {
                                let e = close_of[k];
                                for x in exempt.iter_mut().take(e + 1).skip(k) {
                                    *x = true;
                                }
                                k = e + 1;
                                break;
                            }
                            Tok::Open(_) if close_of[k] != usize::MAX => {
                                let e = close_of[k];
                                for x in exempt.iter_mut().take(e + 1).skip(k) {
                                    *x = true;
                                }
                                k = e + 1;
                            }
                            _ => {
                                exempt[k] = true;
                                k += 1;
                            }
                        }
                    }
                }
                continue;
            }
        }
        // `SomeError::Variant(..)` / `SomeError::Variant { .. }`: an error's diagnostic
        // message — the detail shown after its stable code (ADR 0046 §6).
        if let Tok::Ident(name) = &t[k].0
            && name.ends_with("Error")
            && t.get(k + 1).map(|x| &x.0) == Some(&Tok::Punct(':'))
            && t.get(k + 2).map(|x| &x.0) == Some(&Tok::Punct(':'))
            && matches!(t.get(k + 3).map(|x| &x.0), Some(Tok::Ident(_)))
            && matches!(t.get(k + 4).map(|x| &x.0), Some(Tok::Open('(' | '{')))
            && close_of[k + 4] != usize::MAX
        {
            let e = close_of[k + 4];
            for x in exempt.iter_mut().take(e + 1).skip(k) {
                *x = true;
            }
            k = e + 1;
            continue;
        }
        // `impl Debug for ..`: developer output, never shown in the UI.
        if t[k].0 == Tok::Ident("impl".into()) {
            let mut j = k + 1;
            let mut debug = false;
            while j < t.len() && t[j].0 != Tok::Open('{') && t[j].0 != Tok::Punct(';') {
                if t[j].0 == Tok::Ident("Debug".into())
                    && t.get(j + 1).map(|x| &x.0) == Some(&Tok::Ident("for".into()))
                {
                    debug = true;
                }
                j += 1;
            }
            if debug && j < t.len() && close_of[j] != usize::MAX {
                for x in exempt.iter_mut().take(close_of[j] + 1).skip(k) {
                    *x = true;
                }
                k = close_of[j] + 1;
                continue;
            }
        }
        // Lookups: tr!("key"), trf!("key", ..), tr_key!("key"), tr_str("key"), tr_format("key", ..),
        // l10n::tr("key").
        if let Tok::Ident(name) = &t[k].0 {
            let mac = t.get(k + 1).map(|x| &x.0) == Some(&Tok::Punct('!'));
            let open = if mac { k + 2 } else { k + 1 };
            let is_lookup = match name.as_str() {
                "tr" | "trf" | "tr_key" => true,
                "tr_str" | "tr_format" => !mac,
                _ => false,
            };
            if is_lookup
                && matches!(t.get(open).map(|x| &x.0), Some(Tok::Open(_)))
                && matches!(t.get(open + 1).map(|x| &x.0), Some(Tok::Str { .. }))
            {
                exempt[open + 1] = true;
            }
            // trf_into!(buffer, "key", ..): the key follows the buffer, after its first comma
            // outside any nested group (the buffer may be a call).
            if name == "trf_into"
                && mac
                && matches!(t.get(open).map(|x| &x.0), Some(Tok::Open(_)))
                && close_of[open] != usize::MAX
            {
                let mut c = open + 1;
                while c < close_of[open] {
                    match &t[c].0 {
                        Tok::Open(_) if close_of[c] != usize::MAX => c = close_of[c] + 1,
                        Tok::Punct(',') => break,
                        _ => c += 1,
                    }
                }
                if c < close_of[open] && matches!(t.get(c + 1).map(|x| &x.0), Some(Tok::Str { .. }))
                {
                    exempt[c + 1] = true;
                }
            }
        }
        k += 1;
    }
    // `// l10n-block:` notes: the next brace group after the note's line.
    for nl in &lx.block_notes {
        // A body `{..}`, or an array `= [..]` / `&[..]` (a const table).
        let opens_block = |o: usize| match &t[o].0 {
            Tok::Open('{') => true,
            Tok::Open('[') => {
                o > 1
                    && (t[o - 1].0 == Tok::Punct('=')
                        || t[o - 1].0 == Tok::Punct('&') && t[o - 2].0 == Tok::Punct('='))
            }
            _ => false,
        };
        if let Some(o) = (0..t.len()).find(|o| t[*o].1 > *nl && opens_block(*o))
            && close_of[o] != usize::MAX
        {
            for x in exempt.iter_mut().take(close_of[o] + 1).skip(o) {
                *x = true;
            }
        }
    }
    let mut inside = vec![only.is_none(); t.len()];
    if let Some(names) = only {
        for k in 0..t.len().saturating_sub(1) {
            let named = matches!(&t[k + 1].0, Tok::Ident(n) if names.contains(&n.as_str()));
            if t[k].0 == Tok::Ident("fn".into())
                && named
                && let Some(o) = (k..t.len()).find(|o| t[*o].0 == Tok::Open('{'))
                && close_of[o] != usize::MAX
            {
                for x in inside.iter_mut().take(close_of[o] + 1).skip(o) {
                    *x = true;
                }
            }
        }
    }
    for (k, (tok, _)) in t.iter().enumerate() {
        let Tok::Str { text, line, pos } = tok else {
            continue;
        };
        if exempt[k] || !inside[k] || !is_words(text) {
            continue;
        }
        if lx.notes.iter().any(|n| *n == *line || *n + 1 == *line) {
            continue;
        }
        let col = pos - line_starts.get(line - 1).copied().unwrap_or(0) + 1;
        out.push((*line, col, text.clone()));
    }
    out
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Whether a plugin's `Cargo.toml` declares it a UI plugin (`[package.metadata.forge]` with
/// `ui-plugin = true`): a plugin that builds panels without the `forge-panels-` name.
fn declares_ui(toml: &str) -> bool {
    let mut in_table = false;
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_table = l == "[package.metadata.forge]";
            continue;
        }
        if in_table
            && let Some((k, v)) = l.split_once('=')
            && k.trim() == "ui-plugin"
        {
            return v.trim() == "true";
        }
    }
    false
}

/// The editor's UI code: every first-party panel plugin (and every plugin that declares
/// itself a UI plugin), the shell's UI modules and forge-ui's widgets.
fn ui_sources() -> Vec<PathBuf> {
    let root = repo();
    let mut files = Vec::new();
    let Ok(rd) = std::fs::read_dir(root.join("plugins")) else {
        return files;
    };
    let mut plugins: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("forge-panels-"))
                || std::fs::read_to_string(p.join("Cargo.toml")).is_ok_and(|t| declares_ui(&t))
        })
        .collect();
    plugins.sort();
    for p in plugins {
        rs_files(&p.join("src"), &mut files);
    }
    let ed = root.join("crates/forge-editor/src");
    for m in UI_MODULES {
        let p = ed.join(m);
        if p.is_dir() {
            rs_files(&p, &mut files);
        } else {
            files.push(p);
        }
    }
    rs_files(&root.join("crates/forge-ui/src/widgets"), &mut files);
    // The dock's views (its model, `dock/model.rs`, only reports layout errors: UI-0008 detail).
    rs_files(&root.join("crates/forge-ui/src/dock"), &mut files);
    files.retain(|f| !f.ends_with("dock/model.rs") && !f.ends_with("dock\\model.rs"));
    files.push(root.join("crates/forge-ui/src/overlay.rs"));
    files
}

/// The shell's modules that build or word what the user sees (the rest of forge-editor is
/// the model: commands, stores, validation — their messages reach the UI as the detail of
/// a coded problem, filled into a looked-up template).
const UI_MODULES: &[&str] = &[
    "actions.rs",
    "command_labels.rs",
    "error.rs",
    "keys.rs",
    "notify.rs",
    "overlay.rs",
    "palette.rs",
    "panels.rs",
    "session.rs",
    "settings.rs",
    "shell.rs",
    "stand_in.rs",
    "tips.rs",
    "viewport",
];

/// The methods by which a model type words itself for the UI.
const PRESENTATIONAL: &[&str] = &[
    "label", "title", "word", "line", "describe", "summary", "note", "hint", "caption",
];

#[test]
fn every_ui_string_literal_is_a_localisation_key() {
    let files = ui_sources();
    assert!(files.len() >= 60, "only {} UI source files", files.len());
    let mut problems = Vec::new();
    let mut literals = 0usize;
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        literals += lex(&src)
            .toks
            .iter()
            .filter(|(t, _)| matches!(t, Tok::Str { .. }))
            .count();
        let rel = f.strip_prefix(repo()).unwrap_or(f).display().to_string();
        for (line, col, text) in findings(&src) {
            problems.push(format!(
                "{}:{line}:{col}: \"{text}\"",
                rel.replace('\\', "/")
            ));
        }
    }
    // The model modules of forge-editor: their presentational methods (a licence tier's
    // label, a loss's line) return text the panels show as it is.
    let mut model = Vec::new();
    rs_files(&repo().join("crates/forge-editor/src"), &mut model);
    model.retain(|f| !files.contains(f));
    assert!(model.len() >= 40, "only {} model files", model.len());
    for f in &model {
        let src = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let rel = f.strip_prefix(repo()).unwrap_or(f).display().to_string();
        for (line, col, text) in findings_where(&src, Some(PRESENTATIONAL)) {
            problems.push(format!(
                "{}:{line}:{col}: \"{text}\"",
                rel.replace('\\', "/")
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{} UI string literals are not localisation keys (wrap in tr!/trf!, or tr_key! when \
         looked up where shown; data takes an `// l10n: <why>` note):\n{}",
        problems.len(),
        problems.join("\n")
    );
    println!(
        "ui strings: {} UI files ({literals} string literals) and the presentational methods of \
         {} model files: every worded literal a localisation key",
        files.len(),
        model.len()
    );
}

#[test]
fn positive_control_one_hard_coded_label_is_found() {
    let bad = r#"
        fn build(b: &mut B) {
            b.add(p, "title", NodeStyle::leaf(), Label::new("Hard-coded text"));
            let n = 3;
            let s = format!("{n} assets imported");
            b.add(p, "delete", NodeStyle::leaf(), Button::new("Delete"));
            item.note = Some(format!("{missing} untranslated"));
            let key = format!("row:{path}");
        }
    "#;
    let f = findings(bad);
    let texts: Vec<&str> = f.iter().map(|(_, _, t)| t.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "Hard-coded text",
            "{n} assets imported",
            "Delete",
            "{missing} untranslated"
        ],
        "{f:?}"
    );
    let good = r#"
        fn build(b: &mut B) {
            b.add(p, "title", NodeStyle::leaf(), Label::new(forge_ui::tr!("Hard-coded text")));
            let s = forge_ui::trf!("{n} assets imported", n);
            forge_ui::trf_into!(&mut buf, "{n} assets moved", n);
            forge_ui::trf_into!(line(lines, &mut k), "{n} assets left", n);
            let k = forge_ui::tr_key!("Delete");
            // l10n: a manifest, parsed, never shown
            let m = "Plugin(id: \"x\", kind: Source)";
            let key = "forge.assets";
        }
        // l10n-block: an error's Display, detail next to its code
        impl std::fmt::Display for E {
            fn fmt(&self, f: &mut F) -> R { write!(f, "bad chord here") }
        }
        #[cfg(test)]
        mod tests {
            fn t() { let _ = Label::new("Test only text"); }
        }
    "#;
    assert_eq!(findings(good), Vec::new());
}

#[test]
fn a_plugin_declares_itself_a_ui_plugin_in_its_manifest() {
    assert!(declares_ui(
        "[package]\nname = \"x\"\n\n[package.metadata.forge]\nui-plugin = true\n"
    ));
    assert!(!declares_ui(
        "[package]\nname = \"x\"\n\n[package.metadata.forge]\nui-plugin = false\n"
    ));
    assert!(!declares_ui("[dependencies]\nui-plugin = true\n"));
}
