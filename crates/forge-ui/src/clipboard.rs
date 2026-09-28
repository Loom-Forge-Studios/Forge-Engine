//! The clipboard bridge (Ch.21 §21.9).
//!
//! * Text goes through the platform clipboard ([`OsClipboard`], `arboard`).
//! * Structured editor content (entities, components, graph nodes) is copied as
//!   **RON over reflect** under [`FORGE_RON_MIME`], always with a plain-text fallback.
//! * **Copy is a read. Paste is a command** (§21.18): the clipboard never mutates
//!   project state; the editor turns a paste into an `EditorCommand`.
//!
//! **Backends.** [`OsClipboard`] (feature `os-clipboard`, on by default and in the winit
//! runner) is the real one: text reaches other applications. `arboard`'s Windows backend
//! is BSL-1.0, admitted to the allow-list by D-8 / ADR 0006. Structured content rides
//! alongside: the OS holds the text fallback, and the structured payload is returned only
//! while the OS clipboard still holds the text this process put there (another
//! application's copy replaces both). [`InProcessClipboard`] is the D-4 in-memory
//! implementation, used headless and wherever the OS clipboard cannot be opened (no
//! display server on a CI leg); `backend_name` always says which one is serving.

/// The Forge structured-content MIME type (RON over reflect).
pub const FORGE_RON_MIME: &str = "application/x-forge-ron";

/// A clipboard entry: optional structured data plus the text fallback every entry has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardItem {
    pub text: String,
    /// `(mime, data)`, e.g. `(FORGE_RON_MIME, "(entities: [...])")`.
    pub structured: Option<(String, String)>,
}

pub trait Clipboard {
    fn set(&mut self, item: ClipboardItem);
    fn get(&mut self) -> Option<ClipboardItem>;
    /// Which implementation this is (shown in diagnostics; never hidden, D-4).
    fn backend_name(&self) -> &'static str;

    fn set_text(&mut self, text: &str) {
        self.set(ClipboardItem {
            text: text.to_string(),
            structured: None,
        });
    }
    fn get_text(&mut self) -> Option<String> {
        self.get().map(|i| i.text)
    }
}

/// In-memory clipboard shared by one process (D-4 in-memory implementation).
#[derive(Default)]
pub struct InProcessClipboard {
    item: Option<ClipboardItem>,
}

impl Clipboard for InProcessClipboard {
    fn set(&mut self, item: ClipboardItem) {
        self.item = Some(item);
    }
    fn get(&mut self) -> Option<ClipboardItem> {
        self.item.clone()
    }
    fn backend_name(&self) -> &'static str {
        "in-process (D-4 in-memory)"
    }
}

/// The OS clipboard (`arboard`). Falls back to the in-process clipboard — and says so in
/// [`Clipboard::backend_name`] — when the platform clipboard cannot be opened.
#[cfg(feature = "os-clipboard")]
pub struct OsClipboard {
    os: Option<arboard::Clipboard>,
    open_error: Option<String>,
    /// The structured payload of our last copy, with the text it was paired with.
    structured: Option<(String, (String, String))>,
    fallback: InProcessClipboard,
}

#[cfg(feature = "os-clipboard")]
impl OsClipboard {
    pub fn new() -> Self {
        let (os, open_error) = match arboard::Clipboard::new() {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(e.to_string())),
        };
        Self {
            os,
            open_error,
            structured: None,
            fallback: InProcessClipboard::default(),
        }
    }

    /// Whether the platform clipboard is serving (false: the in-process fallback is).
    pub fn is_os(&self) -> bool {
        self.os.is_some()
    }

    /// Why the platform clipboard could not be opened, if it could not.
    pub fn open_error(&self) -> Option<&str> {
        self.open_error.as_deref()
    }
}

#[cfg(feature = "os-clipboard")]
impl Default for OsClipboard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "os-clipboard")]
impl Clipboard for OsClipboard {
    fn set(&mut self, item: ClipboardItem) {
        let Some(os) = self.os.as_mut() else {
            self.fallback.set(item);
            return;
        };
        self.structured = item.structured.map(|s| (item.text.clone(), s));
        if os.set_text(item.text.clone()).is_err() {
            // The OS refused this copy (clipboard locked by another process): keep it
            // in-process so a paste in Forge still works.
            self.fallback.set(ClipboardItem {
                text: item.text,
                structured: self.structured.as_ref().map(|(_, s)| s.clone()),
            });
        }
    }

    fn get(&mut self) -> Option<ClipboardItem> {
        let Some(os) = self.os.as_mut() else {
            return self.fallback.get();
        };
        match os.get_text() {
            Ok(text) => {
                let structured = self
                    .structured
                    .as_ref()
                    .filter(|(t, _)| *t == text)
                    .map(|(_, s)| s.clone());
                Some(ClipboardItem { text, structured })
            }
            Err(_) => self.fallback.get(),
        }
    }

    fn backend_name(&self) -> &'static str {
        if self.os.is_some() {
            "os (arboard)"
        } else {
            "in-process fallback (the OS clipboard could not be opened)"
        }
    }
}
