// timed-gates: exempt(cold-start times are printed; the asserts are on the work done at construction)
//! `test_ui_bundled_font` (Ch.21 §21.2 "Fonts", D-7 as amended, ADR 0013; gate rows
//! `C-ui-bundled-font` and `C-ui-lazy-font-fallback`).
//!
//! * **`C-ui-bundled-font`.** The bundled Apache-2.0 faces in `crates/forge-ui/fonts`
//!   (the Roboto 2012 set + Droid Sans Mono — D-7 amended: these *are* the bundled set) are
//!   what UI text resolves to on every machine, and the committed display-list goldens were
//!   made with exactly those bytes, so the goldens compare for real anywhere the bundle is.
//!   Positive control: a text system without the bundle resolves other faces and fails.
//! * **`C-ui-lazy-font-fallback`.** Cold start scans **no** system fonts: the system
//!   database is merged in lazily, once, on the first glyph the bundle lacks (CJK, emoji),
//!   and never for Latin UI text. The check observes the font database itself — its face
//!   count, and the merge counter that lives inside the only function that merges — not a
//!   flag the text system reports about itself. Positive control: a `TextSystem` that scans
//!   system fonts at construction (the pre-D-7 behaviour) fails, on any machine, including
//!   one with no system fonts at all (the counter still moves).

use std::time::Instant;

use forge_ui::text::{
    BUNDLED_FACES, BUNDLED_MONO_FAMILY, BUNDLED_SANS_FAMILY, FontConfig, TextStyle, TextSystem,
};

/// Typical UI strings: none of them may cost a system font scan.
const LATIN_UI: &[&str] = &[
    "Save",
    "Cancel",
    "Opacity 80 %",
    "let pos: FramePos = frame.local(1.5, 0.0, -2.25);",
    "Überprüfung — «Größe» 12,5 N·s ± 0.1 °C",
    "File  Edit  View  Window  Help",
];

/// The font database holds the bundle and nothing else, and nothing was ever merged in.
fn only_the_bundle(t: &TextSystem, when: &str) -> Result<(), String> {
    if t.stats.system_font_loads != 0 {
        return Err(format!(
            "the system font database was merged {} time(s) {when}",
            t.stats.system_font_loads
        ));
    }
    if t.face_count() != BUNDLED_FACES.len() {
        return Err(format!(
            "the font database holds {} faces {when}, the bundle has {}",
            t.face_count(),
            BUNDLED_FACES.len()
        ));
    }
    Ok(())
}

fn check_cold_start(cfg: &FontConfig) -> Result<(), String> {
    let mut t = TextSystem::new(cfg);
    only_the_bundle(&t, "at construction")?;
    let body = TextStyle::body(14.0);
    let mono = TextStyle {
        mono: true,
        ..TextStyle::body(13.0)
    };
    for s in LATIN_UI {
        t.layout(s, &body, None);
        t.layout(s, &mono, None);
    }
    only_the_bundle(&t, "after laying out Latin UI text")?;
    Ok(())
}

#[test]
fn cold_start_uses_the_bundle_and_scans_no_system_fonts() {
    let t0 = Instant::now();
    check_cold_start(&FontConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let bundled_ms = t0.elapsed().as_secs_f64() * 1000.0;
    // What the pre-D-7 core paid on every cold start: one full system font scan.
    let scan_ms = forge_ui::text::measure_system_scan().as_secs_f64() * 1000.0;
    println!(
        "test_ui_bundled_font: cold TextSystem + 12 layouts with the bundle {bundled_ms:.1} ms; \
         the system font scan it no longer does at start-up {scan_ms:.1} ms"
    );
}

#[test]
fn positive_control_eager_system_scan_fails() {
    let e = check_cold_start(&FontConfig {
        eager_system_scan: true,
        ..FontConfig::default()
    });
    assert!(
        e.as_ref().is_err_and(|m| m.contains("at construction")),
        "an eager system scan passed the cold-start check: {e:?}"
    );
}

#[test]
fn a_glyph_miss_merges_system_fonts_once() {
    let mut t = TextSystem::new(&FontConfig::default());
    let body = TextStyle::body(14.0);
    t.layout("Latin only", &body, None);
    assert!(!t.system_fonts_loaded());
    assert_eq!(t.face_count(), BUNDLED_FACES.len());
    t.layout("漢字かな交じり文", &body, None);
    assert!(t.system_fonts_loaded(), "a CJK miss did not load fallbacks");
    t.layout("مرحبا بالعالم 🎨", &body, None);
    assert_eq!(t.stats.system_font_loads, 1, "the scan must happen once");
}

#[test]
fn bundled_only_never_scans() {
    let mut t = TextSystem::new(&FontConfig::bundled_only());
    t.layout("漢字 🎨", &TextStyle::body(14.0), None);
    assert!(!t.system_fonts_loaded());
    assert_eq!(t.stats.system_font_loads, 0);
    assert_eq!(t.face_count(), BUNDLED_FACES.len());
}

// ---- C-ui-bundled-font -------------------------------------------------------------------

/// The fingerprint the committed display-list goldens were blessed with.
fn golden_fingerprint() -> Result<(String, u64), String> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/display_lists.ron");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    #[derive(serde::Deserialize)]
    struct G {
        font: String,
        font_hash: u64,
    }
    let g: G = ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((g.font, g.font_hash))
}

/// UI text resolves to the bundled faces, byte for byte the ones the goldens were made with.
fn check_bundle_resolves(cfg: &FontConfig) -> Result<(), String> {
    let t = TextSystem::new(cfg);
    let (names, hash) = t.font_fingerprint();
    let want = format!("{BUNDLED_SANS_FAMILY} + {BUNDLED_MONO_FAMILY}");
    if names != want {
        return Err(format!(
            "UI families resolve to {names:?}, not the bundled {want:?}"
        ));
    }
    let (gname, ghash) = golden_fingerprint()?;
    if (gname.as_str(), ghash) != (names.as_str(), hash) {
        return Err(format!(
            "the resolved faces ({names:?} {hash:#x}) are not the ones the goldens were blessed \
             with ({gname:?} {ghash:#x})"
        ));
    }
    Ok(())
}

#[test]
fn ui_text_resolves_to_the_bundle_the_goldens_were_made_with() {
    check_bundle_resolves(&FontConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    check_bundle_resolves(&FontConfig::bundled_only()).unwrap_or_else(|e| panic!("{e}"));
}

/// W2: without the bundle the UI families resolve to whatever the machine has (or to
/// nothing), and the check fails — so a green check really means "the bundle".
#[test]
fn positive_control_unbundled_text_system_fails() {
    let e = check_bundle_resolves(&FontConfig {
        bundled: Vec::new(),
        system_fallback: true,
        eager_system_scan: true,
    });
    assert!(e.is_err(), "a text system without the bundle passed: {e:?}");
}

#[test]
fn bundled_fingerprint_is_the_same_everywhere() {
    // The fingerprint hashes the resolved faces' bytes: with the bundle it is a constant of
    // the repository, which is what makes the display-list goldens portable.
    let a = TextSystem::new(&FontConfig::bundled_only()).font_fingerprint();
    let b = TextSystem::new(&FontConfig::default()).font_fingerprint();
    assert_eq!(a, b);
}
