//! The keymap (Ch.21 §21.17): chords to actions, per context, rebindable, with conflict
//! detection.
//!
//! * A [`Chord`] is one or two strokes (`Ctrl+Shift+P`, `Ctrl+K Ctrl+S`).
//! * Every binding has a [`KeyContext`]: global → window → panel type → focused widget
//!   role. Resolution walks the focus path from the innermost context outwards, so a text
//!   field's `Delete` wins over the global `Delete` while the field has focus.
//! * **Conflicts are reported, never last-wins.** Binding a chord that another action
//!   already has in an overlapping context — or a chord that is a prefix of another's —
//!   fails with [`EditorError::KeyConflict`] naming both actions. An inner context may
//!   deliberately override an outer binding, but only by naming the action it shadows
//!   ([`KeyMap::bind_shadowing`]), so every override is visible in the data.
//! * Defaults are data ([`KeymapFile`], `data/keymap.ron`), overridden per preset and per
//!   user by layering more files ([`KeyMap::from_layers`]). The keymap is user config, not
//!   project state.

use serde::{Deserialize, Serialize};

use forge_ui::{KeyCode, KeyEvent};

use crate::EditorError;

/// The keymap file format version this build writes and the newest it reads.
pub const KEYMAP_VERSION: u32 = 1;

/// One key press with its modifiers.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyStroke {
    /// Canonical key name: an upper-case character (`P`, `/`) or a named key (`Space`,
    /// `Enter`, `F2`, `Left`, `Delete`).
    pub key: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

const NAMED: &[&str] = &[
    "Tab",
    "Enter",
    "Escape",
    "Space",
    "Backspace",
    "Delete",
    "Left",
    "Right",
    "Up",
    "Down",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Insert",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "ContextMenu",
];

fn canonical_key(k: &str) -> Option<String> {
    if let Some(n) = NAMED.iter().find(|n| n.eq_ignore_ascii_case(k)) {
        return Some((*n).to_string());
    }
    match k.to_ascii_lowercase().as_str() {
        "esc" => return Some("Escape".into()),
        "del" => return Some("Delete".into()),
        "return" => return Some("Enter".into()),
        "plus" => return Some("+".into()),
        _ => {}
    }
    let mut chars = k.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_whitespace() => Some(c.to_uppercase().collect()),
        _ => None,
    }
}

impl KeyStroke {
    /// Parse `Ctrl+Shift+P` (modifier names are case-insensitive; `Cmd`/`Super`/`Win` are
    /// `Meta`, `Control` is `Ctrl`, `Option` is `Alt`).
    pub fn parse(s: &str) -> Result<Self, EditorError> {
        let bad = || EditorError::BadChord(format!("{s:?} is not a key stroke"));
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key, mods) = match parts.split_last() {
            // `Ctrl++` ends in an empty part: the key is `+`.
            Some((last, rest)) if last.is_empty() && rest.last() == Some(&"") => {
                ("+", &rest[..rest.len() - 1])
            }
            Some((last, rest)) => (*last, rest),
            None => return Err(bad()),
        };
        let mut st = KeyStroke {
            key: canonical_key(key).ok_or_else(bad)?,
            ctrl: false,
            shift: false,
            alt: false,
            meta: false,
        };
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => st.ctrl = true,
                "shift" => st.shift = true,
                "alt" | "option" => st.alt = true,
                "meta" | "cmd" | "super" | "win" => st.meta = true,
                _ => return Err(bad()),
            }
        }
        Ok(st)
    }

    /// The stroke a key event is, if its key can be bound.
    pub fn from_event(e: &KeyEvent) -> Option<Self> {
        let key = match &e.code {
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::Enter => "Enter".into(),
            KeyCode::Escape => "Escape".into(),
            KeyCode::Space => "Space".into(),
            KeyCode::Backspace => "Backspace".into(),
            KeyCode::Delete => "Delete".into(),
            KeyCode::Left => "Left".into(),
            KeyCode::Right => "Right".into(),
            KeyCode::Up => "Up".into(),
            KeyCode::Down => "Down".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PageUp".into(),
            KeyCode::PageDown => "PageDown".into(),
            KeyCode::F1 => "F1".into(),
            KeyCode::F2 => "F2".into(),
            KeyCode::F3 => "F3".into(),
            KeyCode::F4 => "F4".into(),
            KeyCode::F5 => "F5".into(),
            KeyCode::F6 => "F6".into(),
            KeyCode::F7 => "F7".into(),
            KeyCode::F8 => "F8".into(),
            KeyCode::F9 => "F9".into(),
            KeyCode::F10 => "F10".into(),
            KeyCode::F11 => "F11".into(),
            KeyCode::F12 => "F12".into(),
            KeyCode::Insert => "Insert".into(),
            KeyCode::ContextMenu => "ContextMenu".into(),
            KeyCode::Char(c) => c.to_uppercase().collect(),
            KeyCode::Other => return None,
        };
        Some(KeyStroke {
            key,
            ctrl: e.mods.ctrl,
            shift: e.mods.shift,
            alt: e.mods.alt,
            meta: e.mods.meta,
        })
    }
}

impl std::fmt::Display for KeyStroke {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl+"),
            (self.shift, "Shift+"),
            (self.alt, "Alt+"),
            (self.meta, "Meta+"),
        ] {
            if on {
                f.write_str(name)?;
            }
        }
        f.write_str(&self.key)
    }
}

/// The name a person reads for a named key, in the UI locale (`Delete` is `Entf` on a
/// German keyboard). Characters and function keys read as they are.
fn key_label(key: &str) -> &str {
    match key {
        "Tab" => forge_ui::tr!("Tab"),
        "Enter" => forge_ui::tr!("Enter"),
        "Escape" => forge_ui::tr!("Escape"),
        "Space" => forge_ui::tr!("Space"),
        "Backspace" => forge_ui::tr!("Backspace"),
        "Delete" => forge_ui::tr!("Delete"),
        "Left" => forge_ui::tr!("Left"),
        "Right" => forge_ui::tr!("Right"),
        "Up" => forge_ui::tr!("Up"),
        "Down" => forge_ui::tr!("Down"),
        "Home" => forge_ui::tr!("Home"),
        "End" => forge_ui::tr!("End"),
        "PageUp" => forge_ui::tr!("PageUp"),
        "PageDown" => forge_ui::tr!("PageDown"),
        "Insert" => forge_ui::tr!("Insert"),
        "ContextMenu" => forge_ui::tr!("ContextMenu"),
        other => other,
    }
}

impl KeyStroke {
    /// The stroke as a person reads it, in the UI locale (`Display` is the stored,
    /// canonical form that keymap files use).
    #[must_use]
    pub fn label(&self) -> String {
        let mut out = String::new();
        for (on, name) in [
            (self.ctrl, forge_ui::tr!("Ctrl")),
            (self.shift, forge_ui::tr!("Shift")),
            (self.alt, forge_ui::tr!("Alt")),
            (self.meta, forge_ui::tr!("Meta")),
        ] {
            if on {
                out.push_str(name);
                out.push('+');
            }
        }
        out.push_str(key_label(&self.key));
        out
    }
}

/// One or two strokes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Chord(pub Vec<KeyStroke>);

impl Chord {
    /// Parse `Ctrl+Shift+P` or the two-stroke `Ctrl+K Ctrl+S`.
    pub fn parse(s: &str) -> Result<Self, EditorError> {
        let strokes: Vec<KeyStroke> = s
            .split_whitespace()
            .map(KeyStroke::parse)
            .collect::<Result<_, _>>()?;
        if strokes.is_empty() || strokes.len() > 2 {
            return Err(EditorError::BadChord(format!(
                "{s:?}: a chord is one or two strokes"
            )));
        }
        Ok(Chord(strokes))
    }

    /// The chord as a person reads it in a menu, the palette or the keybindings editor, in
    /// the UI locale (`Display` is the stored, canonical form).
    #[must_use]
    pub fn label(&self) -> String {
        self.0
            .iter()
            .map(KeyStroke::label)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Whether `self` is a proper prefix of `other` (`Ctrl+K` of `Ctrl+K Ctrl+S`).
    pub fn is_prefix_of(&self, other: &Chord) -> bool {
        self.0.len() < other.0.len() && other.0.starts_with(&self.0)
    }
}

impl std::fmt::Display for Chord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, s) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{s}")?;
        }
        Ok(())
    }
}

impl TryFrom<String> for Chord {
    type Error = EditorError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Chord::parse(&s)
    }
}

impl From<Chord> for String {
    fn from(c: Chord) -> String {
        c.to_string()
    }
}

/// Where a binding applies, outermost to innermost.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum KeyContext {
    /// Everywhere.
    Global,
    /// Anywhere in an editor window (not in a modal dialog of another tool).
    Window,
    /// While focus is inside a panel of this type (`forge.console`).
    Panel(String),
    /// While the focused widget has this AccessKit role (`TextInput`).
    Role(String),
}

impl KeyContext {
    fn depth(&self) -> u8 {
        match self {
            KeyContext::Global => 0,
            KeyContext::Window => 1,
            KeyContext::Panel(_) => 2,
            KeyContext::Role(_) => 3,
        }
    }

    /// Whether both contexts can be active at once (so one chord in both is ambiguous):
    /// the same context; global or window with anything; a panel with a widget role (the
    /// role's widget may sit in that panel). Two different panels, or two different roles,
    /// are never active together.
    pub fn overlaps(&self, other: &KeyContext) -> bool {
        use KeyContext::*;
        match (self, other) {
            (a, b) if a == b => true,
            (Global | Window, _) | (_, Global | Window) => true,
            (Panel(_), Role(_)) | (Role(_), Panel(_)) => true,
            _ => false,
        }
    }

    /// Where the binding applies, as a person reads it in the UI locale (`Display` is the
    /// untranslated form error messages and logs carry). A panel is named by its id; a
    /// widget role by what a person calls the widget, or its AccessKit name when there is
    /// no plainer word for it.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            KeyContext::Global => forge_ui::tr!("every context").into(),
            KeyContext::Window => forge_ui::tr!("editor windows").into(),
            KeyContext::Panel(p) => forge_ui::trf!("the {panel} panel", panel = p),
            KeyContext::Role(r) => {
                let widget = match r.as_str() {
                    "TextInput" => forge_ui::tr!("text field"),
                    "MultilineTextInput" => forge_ui::tr!("multi-line text field"),
                    "SearchInput" => forge_ui::tr!("search field"),
                    // l10n: the patterns are AccessKit role names (identifiers)
                    "Tree" => forge_ui::tr!("tree"),
                    "ListBox" => forge_ui::tr!("list"),
                    // l10n: AccessKit role names (identifiers)
                    "Grid" | "Table" => forge_ui::tr!("table"),
                    other => other,
                };
                forge_ui::trf!("a focused {widget}", widget)
            }
        }
    }
}

impl std::fmt::Display for KeyContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyContext::Global => f.write_str("every context"),
            KeyContext::Window => f.write_str("editor windows"),
            KeyContext::Panel(p) => write!(f, "the {p} panel"),
            KeyContext::Role(r) => write!(f, "a focused {r}"),
        }
    }
}

/// One binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub chord: Chord,
    pub context: KeyContext,
    pub action: String,
    /// The outer-context action this binding deliberately overrides, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadows: Option<String>,
}

/// What a key press resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Run this action.
    Action(String),
    /// The first stroke of a two-stroke chord: wait for the second.
    Pending(KeyStroke),
    /// Nothing is bound (a pending chord that did not complete is dropped too).
    Unbound,
}

/// The keymap (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct KeyMap {
    bindings: Vec<Binding>,
    pending: Option<KeyStroke>,
}

impl KeyMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// The binding that makes `b` ambiguous, if any.
    fn conflict_with(&self, b: &Binding) -> Option<&Binding> {
        self.bindings.iter().find(|x| {
            if !x.context.overlaps(&b.context) {
                return false;
            }
            if x.chord.is_prefix_of(&b.chord) || b.chord.is_prefix_of(&x.chord) {
                return true;
            }
            if x.chord != b.chord {
                return false;
            }
            if x.action == b.action && x.context == b.context {
                return false; // the same binding again
            }
            // A deliberate, named override from a strictly inner context is not ambiguous.
            let inner_shadows_outer = |inner: &Binding, outer: &Binding| {
                inner.context.depth() > outer.context.depth()
                    && inner.shadows.as_deref() == Some(outer.action.as_str())
            };
            !(inner_shadows_outer(b, x) || inner_shadows_outer(x, b))
        })
    }

    fn add(&mut self, b: Binding) -> Result<(), EditorError> {
        if let Some(x) = self.conflict_with(&b) {
            return Err(EditorError::KeyConflict {
                chord: b.chord.to_string(),
                context: b.context.to_string(),
                existing: x.action.clone(),
                existing_context: x.context.to_string(),
                new: b.action,
            });
        }
        if !self.bindings.contains(&b) {
            self.bindings.push(b);
        }
        Ok(())
    }

    /// Bind `chord` in `context` to `action`. A conflict is an error naming both actions.
    pub fn bind(
        &mut self,
        chord: Chord,
        context: KeyContext,
        action: &str,
    ) -> Result<(), EditorError> {
        self.add(Binding {
            chord,
            context,
            action: action.to_string(),
            shadows: None,
        })
    }

    /// Bind `chord` in an inner `context`, deliberately overriding the outer binding of
    /// `shadows` while that context is active.
    pub fn bind_shadowing(
        &mut self,
        chord: Chord,
        context: KeyContext,
        action: &str,
        shadows: &str,
    ) -> Result<(), EditorError> {
        self.add(Binding {
            chord,
            context,
            action: action.to_string(),
            shadows: Some(shadows.to_string()),
        })
    }

    /// Remove the binding of `chord` in `context`. Returns whether there was one.
    pub fn unbind(&mut self, chord: &Chord, context: &KeyContext) -> bool {
        let n = self.bindings.len();
        self.bindings
            .retain(|b| !(b.chord == *chord && b.context == *context));
        self.bindings.len() != n
    }

    /// Give `action` the single chord `chord` in `context` (its other chords there go).
    /// On a conflict nothing changes and the error names both actions.
    pub fn rebind(
        &mut self,
        action: &str,
        context: KeyContext,
        chord: Chord,
    ) -> Result<(), EditorError> {
        let before = self.bindings.clone();
        let shadows = self
            .bindings
            .iter()
            .find(|b| b.action == action && b.context == context)
            .and_then(|b| b.shadows.clone());
        self.bindings
            .retain(|b| !(b.action == action && b.context == context));
        let r = self.add(Binding {
            chord,
            context,
            action: action.to_string(),
            shadows,
        });
        if r.is_err() {
            self.bindings = before;
        }
        r
    }

    /// The chords of `action`, outermost context first (what the palette and menus show).
    pub fn chords_for(&self, action: &str) -> Vec<&Chord> {
        let mut v: Vec<&Binding> = self
            .bindings
            .iter()
            .filter(|b| b.action == action)
            .collect();
        v.sort_by_key(|b| b.context.depth());
        v.into_iter().map(|b| &b.chord).collect()
    }

    /// Every conflict in the map (after loading layers written by hand).
    pub fn audit(&self) -> Vec<EditorError> {
        let mut probe = KeyMap::new();
        let mut out = Vec::new();
        for b in &self.bindings {
            if let Err(e) = probe.add(b.clone()) {
                out.push(e);
            }
        }
        out
    }

    /// Resolve a key press. `path` is the focus path's contexts, innermost first (the
    /// focused widget's role, then its panel); `Window` and `Global` are always active.
    pub fn resolve(&mut self, ev: &KeyEvent, path: &[KeyContext]) -> Resolution {
        let Some(stroke) = KeyStroke::from_event(ev) else {
            return Resolution::Unbound;
        };
        let mut active: Vec<&KeyContext> = path.iter().collect();
        for c in [&KeyContext::Window, &KeyContext::Global] {
            if !active.contains(&c) {
                active.push(c);
            }
        }
        let chord = match self.pending.take() {
            Some(first) => Chord(vec![first, stroke.clone()]),
            None => Chord(vec![stroke.clone()]),
        };
        for ctx in &active {
            if let Some(b) = self
                .bindings
                .iter()
                .find(|b| b.context == **ctx && b.chord == chord)
            {
                return Resolution::Action(b.action.clone());
            }
        }
        if chord.0.len() == 1
            && self
                .bindings
                .iter()
                .any(|b| active.contains(&&b.context) && chord.is_prefix_of(&b.chord))
        {
            self.pending = Some(stroke.clone());
            return Resolution::Pending(stroke);
        }
        Resolution::Unbound
    }

    /// Whether the first stroke of a two-stroke chord is waiting.
    pub fn pending(&self) -> Option<&KeyStroke> {
        self.pending.as_ref()
    }

    /// Build a keymap from the defaults and override layers (preset, then user). Each
    /// layer's `unbind` entries apply first; then each binding **rebinds** its action in
    /// its context. Every conflict is collected (naming both actions) and its binding is
    /// skipped, so a bad user file never silently steals another action's chord.
    pub fn from_layers(
        defaults: &KeymapFile,
        overrides: &[&KeymapFile],
    ) -> (KeyMap, Vec<EditorError>) {
        let mut m = KeyMap::new();
        let mut errs = Vec::new();
        for b in &defaults.bindings {
            if let Err(e) = m.add(b.clone()) {
                errs.push(e);
            }
        }
        for layer in overrides {
            for u in &layer.unbind {
                m.unbind(&u.chord, &u.context);
            }
            for b in &layer.bindings {
                let before = m.bindings.clone();
                m.bindings
                    .retain(|x| !(x.action == b.action && x.context == b.context));
                if let Err(e) = m.add(b.clone()) {
                    m.bindings = before;
                    errs.push(e);
                }
            }
        }
        (m, errs)
    }

    /// The whole map as a file (a user's full keymap export).
    pub fn to_file(&self) -> KeymapFile {
        KeymapFile {
            version: KEYMAP_VERSION,
            bindings: self.bindings.clone(),
            unbind: Vec::new(),
        }
    }
}

/// A chord removed in a context by an override layer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unbind {
    pub chord: Chord,
    pub context: KeyContext,
}

/// A keymap layer as data (`data/keymap.ron`, `presets/<p>/keymap.ron`, the user's file).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeymapFile {
    pub version: u32,
    #[serde(default)]
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub unbind: Vec<Unbind>,
}

impl KeymapFile {
    pub fn from_ron(text: &str) -> Result<Self, EditorError> {
        let f: KeymapFile =
            ron::from_str(text).map_err(|e| EditorError::KeymapFile(e.to_string()))?;
        if f.version == 0 || f.version > KEYMAP_VERSION {
            return Err(EditorError::KeymapFile(format!(
                "version {} is not readable by this editor (it reads 1..={KEYMAP_VERSION})",
                f.version
            )));
        }
        Ok(f)
    }

    pub fn to_ron(&self) -> Result<String, EditorError> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new())
            .map_err(|e| EditorError::KeymapFile(e.to_string()))
    }
}

/// The default keymap shipped with the editor (`data/keymap.ron`).
pub const DEFAULT_KEYMAP: &str = include_str!("../data/keymap.ron");

/// The default keymap layer, parsed.
pub fn default_keymap() -> Result<KeymapFile, EditorError> {
    KeymapFile::from_ron(DEFAULT_KEYMAP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_ui::Modifiers;

    #[test]
    fn strokes_parse_and_print_canonically() {
        let c = Chord::parse("shift+ctrl+p").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.to_string(), "Ctrl+Shift+P");
        let two = Chord::parse("Ctrl+K  ctrl+s").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(two.to_string(), "Ctrl+K Ctrl+S");
        assert!(
            Chord::parse("Ctrl+K")
                .unwrap_or(two.clone())
                .is_prefix_of(&two)
        );
        assert_eq!(
            Chord::parse("Ctrl++").map(|c| c.to_string()).ok(),
            Some("Ctrl++".into())
        );
        for bad in ["", "Ctrl+", "Hyper+X", "A B C", "Ctrl+Nope"] {
            assert!(Chord::parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn key_events_map_to_strokes() {
        let e = KeyEvent::press(
            KeyCode::Char('p'),
            Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::NONE
            },
        );
        assert_eq!(
            KeyStroke::from_event(&e).map(|s| s.to_string()).as_deref(),
            Some("Ctrl+Shift+P")
        );
        let s = KeyEvent::press(KeyCode::Space, Modifiers::SHIFT);
        assert_eq!(
            KeyStroke::from_event(&s).map(|s| s.to_string()).as_deref(),
            Some("Shift+Space")
        );
    }

    #[test]
    fn the_default_keymap_loads_without_conflicts() {
        let f = default_keymap().unwrap_or_else(|e| panic!("{e}"));
        let (m, errs) = KeyMap::from_layers(&f, &[]);
        assert!(errs.is_empty(), "{errs:?}");
        assert!(m.audit().is_empty());
        assert!(!m.chords_for("forge.palette.open").is_empty());
    }
}
