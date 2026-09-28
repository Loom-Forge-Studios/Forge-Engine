//! `test_ui_contrast` (Ch.21 §21.8, §21.23; DoD M2-23).
//!
//! Computes every `(fg, bg)` pair the widgets actually use — recorded by `PaintCx` while
//! the gallery paints, in every theme, with hover/press/keyboard-focus states exercised,
//! plus every state of every button variant — and fails on any pair below its theme's
//! floor (text 4.5:1 AA, 7:1 AAA for high contrast; UI boundaries 3:1).
//!
//! It also enforces "widgets name tokens and never raw colours": no colour literal
//! anywhere under `src/widgets/`.
//!
//! Positive controls: a theme with one low-contrast token, and a widget source with a
//! colour literal, both fail.

use std::collections::BTreeSet;
use std::path::PathBuf;

use forge_ui::gallery::{self, GalleryFaults};
use forge_ui::style::{ColorPair, ColorRole, PairKind, Variant, all_states, button_style};
use forge_ui::testing::Harness;
use forge_ui::{Color, Theme, UiConfig};

/// Paint the gallery in `theme`, exercising interaction states; return the pairs used.
fn used_pairs(theme: Theme) -> BTreeSet<ColorPair> {
    let mut h = Harness::new(UiConfig {
        theme,
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut h.ui, GalleryFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    // Keyboard focus on every interactive widget (focus ring pairs), hover over each.
    for id in &g.interactive {
        h.ui.set_focus(Some(*id), true);
        if let Some(r) = h.ui.rect(*id) {
            h.move_to(r.center());
        }
        h.settle();
    }
    // Every tab page, and a selection inside the text field.
    for i in 0..g.pages.len() {
        h.ui.set_focus(Some(g.tabs), true);
        h.key(forge_ui::KeyCode::Right, forge_ui::Modifiers::NONE);
        h.settle();
        let _ = i;
    }
    h.ui.set_focus(Some(g.text_field), true);
    h.type_text("Selected");
    h.key(forge_ui::KeyCode::Char('a'), forge_ui::Modifiers::CTRL);
    h.settle();
    let mut pairs = h.ui.used_pairs().clone();
    // Button styles in every variant × state (a pressed danger button is rarely painted
    // by a walk, but users see it).
    for v in Variant::ALL {
        for s in all_states() {
            let st = button_style(v, s);
            let bg = st.bg.unwrap_or(ColorRole::BgRaised);
            pairs.insert(ColorPair {
                fg: st.fg,
                bg,
                kind: PairKind::Text,
            });
            if let Some(b) = st.border {
                pairs.insert(ColorPair {
                    fg: b,
                    bg: ColorRole::BgRaised,
                    kind: PairKind::Boundary,
                });
            }
        }
    }
    pairs
}

/// Pairs below the theme's floor, as readable lines.
fn failures(theme: &Theme, pairs: &BTreeSet<ColorPair>) -> Vec<String> {
    pairs
        .iter()
        .filter_map(|p| {
            let c = theme.contrast(*p);
            let floor = theme.floor(p.kind);
            (c + 1e-3 < floor).then(|| {
                format!(
                    "{}: {} on {} ({:?}) = {c:.2}:1 < {floor}:1",
                    theme.name,
                    p.fg.token(),
                    p.bg.token(),
                    p.kind
                )
            })
        })
        .collect()
}

#[test]
fn every_used_pair_meets_its_theme_floor() {
    for theme in Theme::builtins() {
        let pairs = used_pairs(theme.clone());
        // Non-vacuous: text and boundary pairs were both recorded, on several backgrounds.
        assert!(pairs.iter().any(|p| p.kind == PairKind::Text));
        assert!(pairs.iter().any(|p| p.kind == PairKind::Boundary));
        assert!(
            pairs.iter().any(|p| p.fg == ColorRole::FocusRing),
            "no focus ring painted"
        );
        assert!(
            pairs.len() >= 15,
            "{}: only {} pairs",
            theme.name,
            pairs.len()
        );
        let f = failures(&theme, &pairs);
        assert!(f.is_empty(), "contrast failures:\n{}", f.join("\n"));
    }
}

#[test]
fn positive_control_low_contrast_token_fails() {
    let mut theme = Theme::dark();
    let pairs = used_pairs(theme.clone());
    // Muted text almost the colour of raised panels.
    theme.set_color(ColorRole::FgMuted, Color::from_rgba8(0x3a, 0x3c, 0x42, 255));
    let f = failures(&theme, &pairs);
    assert!(
        f.iter().any(|l| l.contains("fg.muted on")),
        "a low-contrast fg.muted passed: {f:?}"
    );
}

// ---- no colour literals under src/widgets/ -------------------------------------------

/// Colour literals in widget code: raw constructors, hex strings, struct literals.
fn colour_literals(files: &[(String, String)]) -> Vec<String> {
    const PATTERNS: &[&str] = &["from_rgba8(", "parse_hex(", "Color {", "Color{", "\"#"];
    let mut bad = Vec::new();
    for (path, text) in files {
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            // A pattern counts only at a word start: `HdrColor {` (the colour picker's data
            // type) is not the theme `Color {` struct literal.
            let hit = PATTERNS.iter().any(|p| {
                code.match_indices(p).any(|(i, _)| {
                    !code[..i]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_')
                })
            });
            if hit {
                bad.push(format!("{path}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    bad
}

fn widget_sources() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/widgets");
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
    {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "rs") {
            out.push((
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                std::fs::read_to_string(&p).unwrap_or_default(),
            ));
        }
    }
    out
}

#[test]
fn no_colour_literal_under_widgets() {
    let files = widget_sources();
    assert!(
        files.len() >= 8,
        "only {} widget files scanned",
        files.len()
    );
    let bad = colour_literals(&files);
    assert!(
        bad.is_empty(),
        "colour literals in widgets/ (use theme tokens):\n{}",
        bad.join("\n")
    );
}

#[test]
fn positive_control_colour_literal_in_a_widget_fails() {
    let mut files = widget_sources();
    files.push((
        "sneaky.rs".into(),
        "fn paint() { let red = Color::from_rgba8(255, 0, 0, 255); }".into(),
    ));
    files.push((
        "sneaky2.rs".into(),
        "fn paint() { let c = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }; }".into(),
    ));
    assert_eq!(colour_literals(&files).len(), 2);
}
