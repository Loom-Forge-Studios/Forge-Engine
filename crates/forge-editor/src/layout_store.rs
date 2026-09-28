//! User layouts: saved and restored by name in the per-user config directory, and the
//! crash-safe autosave of the current layout (Ch.21 §21.17; DoD M2-26).
//!
//! * A layout file is the dock's RON ([`Layout::to_ron`]); unknown panel ids are kept.
//! * [`Autosave`] writes the current layout 2 s after the last change (and at most 10 s
//!   after the first unsaved one, so a long drag cannot postpone it forever), and again on
//!   exit. It is a deadline, not polling: the loop sleeps until [`Autosave::deadline`].

use std::path::PathBuf;
use std::time::Duration;

use forge_ui::dock::Layout;

use crate::EditorError;
use crate::user_config::{read_optional, user_config_dir, write_atomic};

/// The autosave's name (not usable for a named layout).
pub const AUTOSAVE: &str = "autosave";
/// Write this long after the last change.
pub const AUTOSAVE_DEBOUNCE: Duration = Duration::from_secs(2);
/// ...but no later than this after the first unsaved change.
pub const AUTOSAVE_MAX_DELAY: Duration = Duration::from_secs(10);

/// Named layouts in one directory (`<user config>/layouts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutStore {
    dir: PathBuf,
}

fn valid_name(name: &str) -> Result<(), EditorError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        && !name.starts_with(' ')
        && !name.ends_with(' ');
    if ok {
        Ok(())
    } else {
        Err(EditorError::Io(format!(
            "{name:?} is not a layout name (1-64 letters, digits, spaces, - or _)"
        )))
    }
}

impl LayoutStore {
    /// Layouts stored in `dir`.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The user's layouts (`<user config>/layouts`).
    pub fn user() -> Result<Self, EditorError> {
        user_config_dir()
            .map(|d| Self::new(d.join("layouts")))
            .ok_or_else(|| EditorError::Io("no user configuration directory".into()))
    }

    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.ron"))
    }

    /// Save `layout` as `name` (replacing an older one of that name, crash-safely).
    pub fn save(&self, name: &str, layout: &Layout) -> Result<(), EditorError> {
        valid_name(name)?;
        if name == AUTOSAVE {
            return Err(EditorError::Io(format!("{AUTOSAVE:?} is reserved")));
        }
        self.write(name, layout)
    }

    fn write(&self, name: &str, layout: &Layout) -> Result<(), EditorError> {
        let text = layout.to_ron()?;
        write_atomic(&self.path(name), text.as_bytes())
    }

    /// Load the layout `name`; `Ok(None)` if there is none.
    pub fn load(&self, name: &str) -> Result<Option<Layout>, EditorError> {
        valid_name(name)?;
        match read_optional(&self.path(name))? {
            Some(t) => Ok(Some(Layout::from_ron(&t)?)),
            None => Ok(None),
        }
    }

    /// Saved layout names, sorted (the autosave is not listed).
    pub fn list(&self) -> Result<Vec<String>, EditorError> {
        let rd = match std::fs::read_dir(&self.dir) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(EditorError::Io(format!("{}: {e}", self.dir.display()))),
        };
        let mut out: Vec<String> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension()? == "ron")
                    .then(|| p.file_stem()?.to_str().map(str::to_string))
                    .flatten()
            })
            .filter(|n| n != AUTOSAVE && valid_name(n).is_ok())
            .collect();
        out.sort();
        Ok(out)
    }

    /// Write the autosave now.
    pub fn save_autosave(&self, layout: &Layout) -> Result<(), EditorError> {
        self.write(AUTOSAVE, layout)
    }

    /// The last autosaved layout (what the editor restores at start-up).
    pub fn load_autosave(&self) -> Result<Option<Layout>, EditorError> {
        self.load(AUTOSAVE)
    }
}

/// The debounced autosave (see the module docs). Time is the loop's injected clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Autosave {
    first_unsaved: Option<Duration>,
    last_change: Option<Duration>,
    /// Writes so far (tests and diagnostics).
    pub writes: u64,
}

impl Autosave {
    pub fn new() -> Self {
        Self::default()
    }

    /// The layout changed at `now`.
    pub fn changed(&mut self, now: Duration) {
        self.first_unsaved.get_or_insert(now);
        self.last_change = Some(now);
    }

    /// When the next write is due, if anything is unsaved (the loop's `WaitUntil`).
    pub fn deadline(&self) -> Option<Duration> {
        let (first, last) = (self.first_unsaved?, self.last_change?);
        Some((last + AUTOSAVE_DEBOUNCE).min(first + AUTOSAVE_MAX_DELAY))
    }

    /// Write if the deadline has passed. Returns whether it wrote.
    pub fn tick(
        &mut self,
        now: Duration,
        store: &LayoutStore,
        layout: &Layout,
    ) -> Result<bool, EditorError> {
        match self.deadline() {
            Some(d) if now >= d => {
                self.flush(store, layout)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Whether anything is unsaved.
    pub fn pending(&self) -> bool {
        self.first_unsaved.is_some()
    }

    /// Mark everything saved: the same debounce drives other user-config files (editor
    /// settings, the keymap), whose owner writes them and then calls this.
    pub fn saved(&mut self) {
        self.first_unsaved = None;
        self.last_change = None;
        self.writes += 1;
    }

    /// Write now if anything is unsaved (on exit).
    pub fn flush(&mut self, store: &LayoutStore, layout: &Layout) -> Result<(), EditorError> {
        if self.first_unsaved.is_none() {
            return Ok(());
        }
        store.save_autosave(layout)?;
        self.first_unsaved = None;
        self.last_change = None;
        self.writes += 1;
        Ok(())
    }
}
