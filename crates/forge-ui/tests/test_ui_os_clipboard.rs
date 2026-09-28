//! `test_ui_os_clipboard` (Ch.21 §21.9, D-8, ADR 0006; gate row `C-ui-os-clipboard`).
//!
//! A copy through `forge-ui`'s clipboard must reach the **operating system**: an
//! independent `arboard` handle (what every other application sees) reads it back, and a
//! copy made by "another application" is what a paste in Forge receives. Structured
//! content survives only while the OS still holds the text Forge put there.
//!
//! The user's clipboard is saved before and restored after. Where no platform clipboard
//! exists (a CI leg with no display server) the test reports `AWAITING(no OS clipboard)`
//! and never passes silently (W9).
//!
//! Positive control: the in-process clipboard (a backend that never reaches the OS) fails.

#![cfg(feature = "os-clipboard")]

use std::sync::Mutex;

use forge_ui::clipboard::{
    Clipboard, ClipboardItem, FORGE_RON_MIME, InProcessClipboard, OsClipboard,
};

/// Tests in this file share one OS clipboard. Under `cargo test` this mutex serialises them;
/// nextest runs each test in its own process, so `.config/nextest.toml` puts this binary in
/// the one-at-a-time `os-clipboard` group.
static OS: Mutex<()> = Mutex::new(());

fn external() -> Option<arboard::Clipboard> {
    match arboard::Clipboard::new() {
        Ok(c) => Some(c),
        Err(e) => {
            println!("test_ui_os_clipboard: AWAITING(no OS clipboard: {e})");
            None
        }
    }
}

/// Run `f` with the user's clipboard text saved and restored around it.
fn preserving<R>(f: impl FnOnce(&mut arboard::Clipboard) -> R) -> Option<R> {
    let _g = OS.lock().unwrap_or_else(|p| p.into_inner());
    let mut ext = external()?;
    let saved = ext.get_text().ok();
    let r = f(&mut ext);
    if let Some(s) = saved {
        let _ = ext.set_text(s);
    }
    Some(r)
}

/// The property: `clip` writes reach the OS, and OS writes reach `clip`.
fn check_reaches_os(clip: &mut dyn Clipboard, ext: &mut arboard::Clipboard) -> Result<(), String> {
    let ours = format!("forge-ui clipboard probe {}", std::process::id());
    clip.set_text(&ours);
    let seen = ext.get_text().map_err(|e| e.to_string())?;
    if seen != ours {
        return Err(format!(
            "{}: another application reads {seen:?}, not the copied {ours:?}",
            clip.backend_name()
        ));
    }
    let theirs = format!("copied by another application {}", std::process::id());
    ext.set_text(theirs.clone()).map_err(|e| e.to_string())?;
    let got = clip.get_text();
    if got.as_deref() != Some(theirs.as_str()) {
        return Err(format!(
            "{}: a paste reads {got:?}, not the other application's {theirs:?}",
            clip.backend_name()
        ));
    }
    Ok(())
}

#[test]
fn copy_and_paste_reach_the_os() {
    let r = preserving(|ext| {
        let mut clip = OsClipboard::new();
        assert!(clip.is_os(), "{:?}", clip.open_error());
        check_reaches_os(&mut clip, ext)
    });
    if let Some(r) = r {
        r.unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn positive_control_in_process_clipboard_fails() {
    let r = preserving(|ext| check_reaches_os(&mut InProcessClipboard::default(), ext));
    if let Some(r) = r {
        assert!(r.is_err(), "the in-process clipboard passed the OS check");
    }
}

#[test]
fn structured_content_rides_with_its_text_and_dies_with_it() {
    let r = preserving(|ext| {
        let mut clip = OsClipboard::new();
        let item = ClipboardItem {
            text: "Cube, Light".into(),
            structured: Some((FORGE_RON_MIME.into(), "(entities: [1, 2])".into())),
        };
        clip.set(item.clone());
        assert_eq!(clip.get(), Some(item), "structured content lost");
        let _ = ext.set_text("plain text from elsewhere");
        let got = clip.get();
        assert_eq!(
            got.and_then(|i| i.structured),
            None,
            "another application's copy must replace the structured payload"
        );
    });
    let _ = r;
}
