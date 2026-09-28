//! `forge-editor` errors. Every variant carries a stable code allocated in
//! `docs/error-codes.md` (Ch.1.2, W8).

/// Editor shell errors.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditorError {
    /// EDITOR-0001: a chord would be bound twice in overlapping contexts (or one binding is
    /// a prefix of another). Both actions are named; nothing is silently last-wins.
    KeyConflict {
        chord: String,
        context: String,
        existing: String,
        existing_context: String,
        new: String,
    },
    /// EDITOR-0002: a chord's text does not parse (`Ctrl+Shift+P`, `Ctrl+K Ctrl+S`).
    BadChord(String),
    /// EDITOR-0003: no action has this id.
    UnknownAction(String),
    /// EDITOR-0004: the action exists but is disabled in the current context.
    ActionDisabled(String),
    /// EDITOR-0005: a workspace preset manifest or its files are missing or invalid.
    Preset(String),
    /// EDITOR-0006: reading or writing a user configuration file failed.
    Io(String),
    /// EDITOR-0007: a layout file is invalid (wraps the dock's UI-0008 message).
    Layout(String),
    /// EDITOR-0008: a keymap file does not parse or is from a newer editor.
    KeymapFile(String),
    /// EDITOR-0009: a settings type cannot be rendered (not a reflected struct, or a field
    /// type the settings window has no editor for).
    Settings(String),
    /// EDITOR-0010: the editor's plugins did not load (a manifest, a conflict, a
    /// capability); carries the plugin kernel's message.
    Plugin(String),
    /// EDITOR-0011: the play core refused a play control, an input or a replay (the
    /// `SIM-*` error follows).
    Play(String),
    /// EDITOR-0012: a remote pairing or exposure change was refused (a wrong or expired
    /// pairing code, LAN exposure without the explicit confirmation, an unknown device).
    Remote(String),
    /// EDITOR-0013: the plugin index or the downloaded-plugin cache failed (no such plugin
    /// or version, a fetch or a cache write failed, a cached plugin does not load).
    PluginIndex(String),
    /// EDITOR-0014: a panel refused an input before sending any command (an empty or
    /// duplicate name, a value out of range, a step the current state does not allow).
    Refused { what: String, why: String },
    /// EDITOR-0015: a problem was shown without a well-formed code, a description or a next
    /// step (an editor defect; the notification centre shows it anyway and counts it).
    Unexplained(String),
}

impl EditorError {
    /// The stable error code.
    pub fn code(&self) -> &'static str {
        match self {
            EditorError::KeyConflict { .. } => "EDITOR-0001",
            EditorError::BadChord(_) => "EDITOR-0002",
            EditorError::UnknownAction(_) => "EDITOR-0003",
            EditorError::ActionDisabled(_) => "EDITOR-0004",
            EditorError::Preset(_) => "EDITOR-0005",
            EditorError::Io(_) => "EDITOR-0006",
            EditorError::Layout(_) => "EDITOR-0007",
            EditorError::KeymapFile(_) => "EDITOR-0008",
            EditorError::Settings(_) => "EDITOR-0009",
            EditorError::Plugin(_) => "EDITOR-0010",
            EditorError::Play(_) => "EDITOR-0011",
            EditorError::Remote(_) => "EDITOR-0012",
            EditorError::PluginIndex(_) => "EDITOR-0013",
            EditorError::Refused { .. } => "EDITOR-0014",
            EditorError::Unexplained(_) => "EDITOR-0015",
        }
    }

    /// What the user can do next (M2-70: every error shown says so).
    pub fn next_step(&self) -> &'static str {
        match self {
            EditorError::KeyConflict { .. } => {
                forge_ui::tr!("Rebind one of the two actions in the keybindings editor.")
            }
            EditorError::BadChord(_) => {
                forge_ui::tr!("Write the chord as keys joined by +, like Ctrl+Shift+P.")
            }
            EditorError::UnknownAction(_) => {
                forge_ui::tr!(
                    "Enable the plugin that provides the action, or pick another in the palette."
                )
            }
            EditorError::ActionDisabled(_) => {
                forge_ui::tr!("Select what the action works on, or open the panel it belongs to.")
            }
            EditorError::Preset(_) => {
                forge_ui::tr!("Reinstall the preset, or pick another preset.")
            }
            EditorError::Io(_) => {
                forge_ui::tr!(
                    "Check that the user config folder exists and is writable; the defaults are used meanwhile."
                )
            }
            EditorError::Layout(_) => forge_ui::tr!("Reset the layout from the Window menu."),
            EditorError::KeymapFile(_) => {
                forge_ui::tr!(
                    "Fix or delete the keymap file; the default keymap is used meanwhile."
                )
            }
            EditorError::Settings(_) => {
                forge_ui::tr!("Report the settings type named here to its plugin's author.")
            }
            EditorError::Plugin(_) => {
                forge_ui::tr!("Disable or update the plugin named here in the plugin manager.")
            }
            EditorError::Play(_) => forge_ui::tr!("Stop the play session and start it again."),
            EditorError::Remote(_) => {
                forge_ui::tr!("Ask for a new pairing code and connect again.")
            }
            EditorError::PluginIndex(_) => {
                forge_ui::tr!("Check the plugin id and version, then try again.")
            }
            EditorError::Refused { .. } => {
                forge_ui::tr!("Correct what the message names and try again.")
            }
            EditorError::Unexplained(_) => {
                forge_ui::tr!("Report this code with what you were doing.")
            }
        }
    }

    #[doc(hidden)]
    pub fn all_variants_for_tests() -> Vec<EditorError> {
        vec![
            EditorError::KeyConflict {
                chord: String::new(),
                context: String::new(),
                existing: String::new(),
                existing_context: String::new(),
                new: String::new(),
            },
            EditorError::BadChord(String::new()),
            EditorError::UnknownAction(String::new()),
            EditorError::ActionDisabled(String::new()),
            EditorError::Preset(String::new()),
            EditorError::Io(String::new()),
            EditorError::Layout(String::new()),
            EditorError::KeymapFile(String::new()),
            EditorError::Settings(String::new()),
            EditorError::Plugin(String::new()),
            EditorError::Play(String::new()),
            EditorError::Remote(String::new()),
            EditorError::PluginIndex(String::new()),
            EditorError::Refused {
                what: String::new(),
                why: String::new(),
            },
            EditorError::Unexplained(String::new()),
        ]
    }
}

// l10n-block: an error's diagnostic message: the detail shown after its stable code, next to
// the looked-up "what" and next step (ADR 0046 §6)
impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code();
        match self {
            EditorError::KeyConflict {
                chord,
                context,
                existing,
                existing_context,
                new,
            } => write!(
                f,
                "{code}: {chord} in {context} would run \u{201c}{new}\u{201d}, but it already runs \
                 \u{201c}{existing}\u{201d} in {existing_context}; rebind one of them"
            ),
            EditorError::BadChord(m) => write!(f, "{code}: bad chord: {m}"),
            EditorError::UnknownAction(a) => write!(f, "{code}: no action {a}"),
            EditorError::ActionDisabled(a) => write!(f, "{code}: {a} is not available here"),
            EditorError::Preset(m) => write!(f, "{code}: preset: {m}"),
            EditorError::Io(m) => write!(f, "{code}: user config: {m}"),
            EditorError::Layout(m) => write!(f, "{code}: layout: {m}"),
            EditorError::KeymapFile(m) => write!(f, "{code}: keymap file: {m}"),
            EditorError::Settings(m) => write!(f, "{code}: settings: {m}"),
            EditorError::Plugin(m) => write!(f, "{code}: plugins: {m}"),
            EditorError::Play(m) => write!(f, "{code}: play: {m}"),
            EditorError::Remote(m) => write!(f, "{code}: remote: {m}"),
            EditorError::PluginIndex(m) => write!(f, "{code}: plugin index: {m}"),
            EditorError::Refused { what, why } => write!(f, "{code}: {what}: {why}"),
            EditorError::Unexplained(m) => write!(f, "{code}: unexplained problem: {m}"),
        }
    }
}

impl std::error::Error for EditorError {}

impl From<forge_plugin::PluginError> for EditorError {
    fn from(e: forge_plugin::PluginError) -> Self {
        EditorError::Plugin(e.to_string())
    }
}

impl From<forge_ui::UiError> for EditorError {
    fn from(e: forge_ui::UiError) -> Self {
        EditorError::Layout(e.to_string())
    }
}
