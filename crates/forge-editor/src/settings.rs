//! The settings window's model (Ch.21 §21.18, §21.21 "Settings", DoD M2-43): pages
//! **generated from reflection** (Ch.6 §6.2 inspector metadata).
//!
//! * **Editor settings** ([`EditorSettings`]) are user config: theme, UI scale, reduced
//!   motion, caret blink. They live in the per-user config directory, never in the project,
//!   and change directly (they are not project state, I7's allow-list names the writer).
//! * **Project settings** are project state: every edit is an `EditorCommand::SetSetting`
//!   on the bus — undoable, provenance-tagged, visible to automation sessions. Any `#[forge_api]`
//!   struct registered as a settings type renders as a page; WP-U4 ships [`ProjectEditorSettings`]
//!   (grid and snapping defaults the whole team shares), WP-U7 adds the Project page's lifecycle
//!   type, WP-U9 the automation policy row.
//!
//! A page is a list of [`SettingRow`]s: the resolved inspector field (label, tooltip,
//! units, bounds, editor hint), the setting key, and the default from the type's
//! `Default` impl read through reflection.

use forge_cmd::Value;
use forge_reflect::bevy_reflect::{
    PartialReflect, ReflectMut, ReflectRef,
    enums::{DynamicEnum, DynamicVariant},
    structs::Struct,
};
use forge_reflect::{ForgeRegistry, InspectorField, PinKind, forge_api};
use serde::{Deserialize, Serialize};

use crate::EditorError;
use crate::user_config::{read_optional, user_config_dir, write_atomic};

/// The editor's colour theme (user config).
#[forge_api]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditorTheme {
    /// The default dark theme.
    #[default]
    Dark,
    /// A light theme.
    Light,
    /// Maximum contrast (WCAG AAA text).
    HighContrast,
}

impl EditorTheme {
    /// The `forge-ui` theme id.
    pub fn theme_id(self) -> &'static str {
        match self {
            EditorTheme::Dark => "forge.dark",
            EditorTheme::Light => "forge.light",
            EditorTheme::HighContrast => "forge.high_contrast",
        }
    }
    pub fn from_theme_id(id: &str) -> Option<Self> {
        [Self::Dark, Self::Light, Self::HighContrast]
            .into_iter()
            .find(|t| t.theme_id() == id)
    }
    /// The theme's tokens.
    pub fn theme(self) -> forge_ui::Theme {
        match self {
            EditorTheme::Dark => forge_ui::Theme::dark(),
            EditorTheme::Light => forge_ui::Theme::light(),
            EditorTheme::HighContrast => forge_ui::Theme::high_contrast(),
        }
    }
}

/// How many GPUs the engine uses (owner decision 2026-09-25, ADR 0054): an editor
/// preference and a project setting (the exported game's). The adapter pool is built at
/// startup, so a change applies at the next start.
#[forge_api]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuModeSetting {
    /// One GPU: the best one (the default).
    #[default]
    Single,
    /// Every GPU (experimental: moving work between GPUs is unmeasured, M1-2).
    Multi,
}

impl GpuModeSetting {
    /// The stable id (`forge_gpu::GpuMode::id`): `"single"` / `"multi"`.
    pub fn id(self) -> &'static str {
        match self {
            GpuModeSetting::Single => "single",
            GpuModeSetting::Multi => "multi",
        }
    }

    /// From a project-setting value (the variant name, as the settings window writes it,
    /// or the id); anything else is the default, Single.
    pub fn from_value(v: Option<&Value>) -> Self {
        match v {
            Some(Value::Text(t)) if t.eq_ignore_ascii_case("multi") => GpuModeSetting::Multi,
            _ => GpuModeSetting::Single,
        }
    }
}

/// Editor settings: per user, never in the project (Ch.21 §21.18 "user config").
#[forge_api(name = "Editor", category = "Editor")]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    /// The colour theme.
    pub theme: EditorTheme,
    /// Interface size, on top of each monitor's scale factor.
    #[forge(name = "UI scale", units = "%", range = 50..=300, step = 10, widget = "slider")]
    pub ui_scale: f64,
    /// Replace motion with instant changes (animations take no time).
    pub reduced_motion: bool,
    /// Blink the text caret for a few seconds after typing (off: always solid).
    pub caret_blink: bool,
    /// A theme a plugin registered (its `Theme` id), chosen over `theme` while that
    /// plugin is loaded. Empty: `theme`.
    #[forge(hidden)]
    pub theme_override: String,
    /// Show a short tip the first time each panel opens (off: never show tips).
    #[forge(name = "First-run tips")]
    pub first_run_tips: bool,
    /// The panels whose first-run tip was dismissed (comma-separated panel ids).
    #[forge(hidden)]
    pub tips_seen: String,
    /// Use one GPU (the best one) or every GPU. Applies at next start. Multi is
    /// experimental: moving work between GPUs is unmeasured until a second physical GPU is
    /// tested (M1-2).
    #[forge(name = "GPU mode", category = "Graphics")]
    pub gpu_mode: GpuModeSetting,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            theme: EditorTheme::Dark,
            ui_scale: 100.0,
            reduced_motion: false,
            caret_blink: true,
            theme_override: String::new(),
            first_run_tips: true,
            tips_seen: String::new(),
            gpu_mode: GpuModeSetting::Single,
        }
    }
}

/// The file editor settings live in, under the user config directory.
pub const EDITOR_SETTINGS_FILE: &str = "editor_settings.ron";

impl EditorSettings {
    /// Whether the first-run tip of `panel` was dismissed.
    pub fn tip_seen(&self, panel: &str) -> bool {
        self.tips_seen.split(',').any(|p| p == panel)
    }

    /// Remember that `panel`'s tip was dismissed.
    pub fn mark_tip_seen(&mut self, panel: &str) {
        if !self.tip_seen(panel) {
            if !self.tips_seen.is_empty() {
                self.tips_seen.push(',');
            }
            self.tips_seen.push_str(panel);
        }
    }

    /// Show every panel's tip again (Settings → Reset tips).
    pub fn reset_tips(&mut self) {
        self.tips_seen.clear();
    }

    /// The UI scale as a factor (0.5–3.0).
    pub fn scale_factor(&self) -> f32 {
        (self.ui_scale / 100.0).clamp(0.5, 3.0) as f32
    }

    /// Load from `dir` (missing file: defaults; a bad file is an error the shell reports
    /// and then uses defaults).
    pub fn load(dir: &std::path::Path) -> Result<Self, EditorError> {
        match read_optional(&dir.join(EDITOR_SETTINGS_FILE))? {
            Some(t) => {
                ron::from_str(&t).map_err(|e| EditorError::Io(format!("editor settings: {e}")))
            }
            None => Ok(Self::default()),
        }
    }

    /// Save to `dir`, crash-safely.
    pub fn save(&self, dir: &std::path::Path) -> Result<(), EditorError> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new())
            .map_err(|e| EditorError::Io(format!("editor settings: {e}")))?;
        write_atomic(&dir.join(EDITOR_SETTINGS_FILE), text.as_bytes())
    }

    /// The user's config directory, if there is one.
    pub fn user_dir() -> Option<std::path::PathBuf> {
        user_config_dir()
    }
}

/// Up axis of the project's editor views.
#[forge_api]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpAxis {
    /// Y up.
    #[default]
    Y,
    /// Z up.
    Z,
}

/// Editor defaults the whole team shares: they are project settings, edited through
/// commands (`editor.*` keys).
#[forge_api(name = "Grid and snapping", category = "Project")]
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectEditorSettings {
    /// Spacing of the viewport grid.
    #[forge(units = "m", range = 0.1..=100.0, step = 0.1, widget = "slider")]
    pub grid_size: f64,
    /// Snap translation to the grid while dragging.
    pub snap_translate: bool,
    /// Rotation snapping increment.
    #[forge(name = "Rotation snap", units = "deg", range = 1..=90, step = 1, widget = "slider")]
    pub snap_rotate: f64,
    /// Which axis points up in editor views.
    pub up_axis: UpAxis,
    /// Name given to newly created entities.
    pub new_entity_name: String,
}

impl Default for ProjectEditorSettings {
    fn default() -> Self {
        Self {
            grid_size: 1.0,
            snap_translate: false,
            snap_rotate: 15.0,
            up_axis: UpAxis::Y,
            // l10n: a default entity name is project data (the user renames it)
            new_entity_name: "Entity".into(),
        }
    }
}

/// The key prefix of [`ProjectEditorSettings`].
pub const PROJECT_EDITOR_PREFIX: &str = "editor";

/// The settings window's **Project** page (WP-U7, DoD M2-47): the project's name and
/// version (edited through commands like any project setting), and read-only rows for the
/// linked remote, the store and the preset — each opens the dialog that changes it, so a
/// lossy change keeps its one confirmation path.
#[forge_api(name = "Project", category = "Project")]
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectLifecycleSettings {
    /// The project's name: the launcher, the status bar and builds show it.
    pub name: String,
    /// The product version a build stamps into the executable's metadata.
    pub version: String,
    /// The linked remote. Link or change it in Revision history (the URL is checked).
    #[forge(name = "Linked remote", read_only)]
    pub remote: String,
    /// Where the project is stored, and by which store backend.
    #[forge(name = "Store", read_only)]
    pub store_backend: String,
    /// The workspace preset. Change it in the preset switcher: a lossy change says what
    /// is lost and asks first.
    #[forge(read_only)]
    pub preset: String,
    /// What every new automation session may do before a person grants more (the project's
    /// default automation capability policy). Only a person changes it, with a security
    /// command; network, process, file-write and destructive capabilities are never defaults.
    #[forge(name = "Default automation capabilities", read_only)]
    pub automation_policy: String,
}

impl Default for ProjectLifecycleSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: "0.1.0".into(),
            remote: String::new(),
            store_backend: String::new(),
            preset: "3d".into(),
            automation_policy: String::new(),
        }
    }
}

/// The key prefix of [`ProjectLifecycleSettings`] (`project.name`, `project.version`).
pub const PROJECT_LIFECYCLE_PREFIX: &str = "project";

/// The Project page.
pub fn project_lifecycle_page() -> Result<SettingsPage, EditorError> {
    page_for(
        forge_ui::tr_key!("Project"),
        &ProjectLifecycleSettings::default(),
        Some(PROJECT_LIFECYCLE_PREFIX),
    )
}

/// The project's Graphics settings: what the exported game does (edited through commands,
/// `graphics.*` keys; the packager writes them into the game's `forge.json`).
#[forge_api(name = "Graphics", category = "Project")]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectGraphicsSettings {
    /// Whether the exported game uses one GPU (the best one) or every GPU. Applies when the
    /// game next starts. Multi is experimental: moving work between GPUs is unmeasured until
    /// a second physical GPU is tested (M1-2).
    #[forge(name = "GPU mode")]
    pub gpu_mode: GpuModeSetting,
}

/// The key prefix of [`ProjectGraphicsSettings`].
pub const PROJECT_GRAPHICS_PREFIX: &str = "graphics";

/// The project setting an exported game's GPU mode is read from.
pub use forge_project::GPU_MODE_SETTING;

/// The project's Graphics page.
pub fn project_graphics_page() -> Result<SettingsPage, EditorError> {
    page_for(
        forge_ui::tr_key!("Graphics"),
        &ProjectGraphicsSettings::default(),
        Some(PROJECT_GRAPHICS_PREFIX),
    )
}

/// Whether a change to the row `key` takes effect only when the program next starts (the
/// settings window says so, and posts "Applies at next start" when it changes): the GPU
/// mode — the adapter pool is built once, at startup, and nothing restarts silently.
pub fn applies_at_next_start(key: &str) -> bool {
    key == "gpu_mode" || key == GPU_MODE_SETTING
}

/// The note under a settings row (a localisation key), for rows that need more than their
/// tooltip.
pub fn row_note(key: &str) -> Option<&'static str> {
    match key {
        "gpu_mode" => Some(forge_ui::tr_key!(
            "Applies at next start: the editor picks its GPUs when it starts. Multi is \
             experimental \u{2014} moving work between GPUs is unmeasured until a second \
             physical GPU is tested (M1-2, ADR 0020)."
        )),
        GPU_MODE_SETTING => Some(forge_ui::tr_key!(
            "Applies when the exported game next starts. Multi is experimental \u{2014} \
             moving work between GPUs is unmeasured until a second physical GPU is tested \
             (M1-2, ADR 0020)."
        )),
        _ => None,
    }
}

/// How a choice row shows `variant` (a localisation key), when that is more than the
/// variant's name.
pub fn variant_label(key: &str, variant: &str) -> Option<&'static str> {
    if !applies_at_next_start(key) {
        return None;
    }
    // l10n: the match arms are variant identifiers; the labels are the keys.
    match variant {
        // l10n: variant identifier
        "Single" => Some(forge_ui::tr_key!("Single (one GPU)")),
        // l10n: variant identifier
        "Multi" => Some(forge_ui::tr_key!("Multi (every GPU, experimental)")),
        _ => None,
    }
}

/// What a read-only Project row shows, from the mirror (and the lifecycle status).
pub fn lifecycle_value(key: &str, m: &crate::mirror::ProjectMirror) -> String {
    let text = |k: &str| match m.setting(k) {
        Some(Value::Text(t)) if !t.is_empty() => Some(t.clone()),
        _ => None,
    };
    match key {
        "project.remote" => text(crate::project::REMOTE_SETTING).unwrap_or_else(|| {
            forge_ui::tr!("Not linked \u{2014} the project is on this computer only").into()
        }),
        "project.store_backend" => match m.project().and_then(|s| s.open.as_ref()) {
            Some(o) if o.in_memory => forge_ui::trf!(
                "{location} ({backend}, in memory: not kept after exit)",
                location = o.location,
                backend = o.backend
            ),
            Some(o) => forge_ui::trf!(
                "{location} ({backend})",
                location = o.location,
                backend = o.backend
            ),
            None => forge_ui::tr!("No project is open").into(),
        },
        "project.preset" => {
            let dir = text(forge_project::PRESET_SETTING).unwrap_or_else(|| "3d".into());
            crate::presets::Family::from_dir(&dir)
                .map_or(dir, |f| forge_ui::l10n::tr_str(f.label()).into_owned())
        }
        "project.automation_policy" => {
            let p = crate::security::automation_policy(|k| m.setting(k));
            let set = p.is_some();
            let caps: Vec<String> = p
                .unwrap_or_else(crate::security::standard_automation_policy)
                .capabilities()
                .map(|c| c.to_string())
                .collect();
            let caps = if caps.is_empty() {
                forge_ui::tr!("Nothing").to_string()
            } else {
                caps.join(", ")
            };
            if set {
                caps
            } else {
                forge_ui::trf!("{caps} (the editor's default)", caps)
            }
        }
        other => text(other).unwrap_or_default(),
    }
}

// ---- links plugins add to settings rows (the SettingLink point) -------------------------------

/// A link a plugin adds beside a settings row (WP-47): a button that opens the panel where the
/// row's value is changed, when that panel is the plugin's. The registry key is the row's
/// setting key (`project.remote`), so a row carries at most one contributed link, and a
/// plugin can replace, remove or chain another plugin's link like any registry item (I16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingLink {
    /// The panel the button opens (its extension-point id).
    pub panel: String,
    /// The button's text: a localisation key, looked up when the row is built (M2-31).
    pub label: String,
}

/// The `SettingLink` extension point (`forge.editor.setting_link`, WP-47): what
/// [`row_link`] offers beside a row the editor itself has no dialog for.
pub struct SettingLinkPoint;

impl forge_plugin::ExtensionPoint for SettingLinkPoint {
    type Item = SettingLink;
    const ID: &'static str = "forge.editor.setting_link";
    const NAME: &'static str = "SettingLink";
}

/// The links the loaded plugins added, by setting key (taken from the plugin registry when the
/// shell is assembled: [`crate::services::EditorServices::setting_links`]).
pub type SettingLinks = forge_plugin::Registry<SettingLinkPoint>;

/// The button beside the settings row `key`: `(panel id, label key)`. The editor's own dialog
/// ([`lifecycle_dialog`]) comes first — the editor owns its rows' dialogs — then a link a
/// plugin added for the row, if one did.
#[must_use]
pub fn row_link(key: &str, links: &SettingLinks) -> Option<(String, String)> {
    if let Some((panel, label)) = lifecycle_dialog(key) {
        return Some((panel.to_string(), label.to_string()));
    }
    links.get(key).map(|l| (l.panel.clone(), l.label.clone()))
}

/// The dialog a read-only Project row opens: `(panel id, button label)`.
pub fn lifecycle_dialog(key: &str) -> Option<(&'static str, &'static str)> {
    match key {
        "project.remote" => Some(("forge.history", forge_ui::tr_key!("Link a remote\u{2026}"))),
        "project.preset" => Some(("forge.presets", forge_ui::tr_key!("Change preset\u{2026}"))),
        _ => None,
    }
}

/// One row of a settings page.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingRow {
    pub field: InspectorField,
    /// The project-setting key (`editor.grid_size`), or the field name for user config.
    pub key: String,
    pub default: Value,
    /// For enum fields: the variant names.
    pub variants: Vec<String>,
}

impl SettingRow {
    /// How the row is edited.
    pub fn editor(&self) -> RowEditor {
        match self.field.kind {
            PinKind::Bool => RowEditor::Toggle,
            PinKind::Enum => RowEditor::Choice,
            PinKind::Int | PinKind::Float
                if self.field.min.is_some()
                    && self.field.max.is_some()
                    && self.field.widget == Some("slider") =>
            {
                RowEditor::Slider
            }
            PinKind::Int | PinKind::Float => RowEditor::Number,
            _ => RowEditor::Text,
        }
    }

    /// The value shown for `current` (the setting, if set) — the default otherwise.
    pub fn shown<'a>(&'a self, current: Option<&'a Value>) -> &'a Value {
        current.unwrap_or(&self.default)
    }
}

/// The editor widget a row gets.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RowEditor {
    Toggle,
    /// A slider: a drag is one gesture, one undo entry.
    Slider,
    /// A numeric field: one command per committed value.
    Number,
    Choice,
    Text,
}

/// One page of the settings window, generated from a reflected settings type.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsPage {
    pub title: String,
    /// `(group, rows)`, in the type's category order.
    pub groups: Vec<(Option<String>, Vec<SettingRow>)>,
    /// Project state (edited through commands) or user config.
    pub project: bool,
}

impl SettingsPage {
    /// Every row, in order.
    pub fn rows(&self) -> impl Iterator<Item = &SettingRow> {
        self.groups.iter().flat_map(|(_, r)| r.iter())
    }
}

/// Read a reflected field as a `Value` (the settings value model).
pub fn value_of(field: &dyn PartialReflect) -> Option<Value> {
    if let Some(b) = field.try_downcast_ref::<bool>() {
        return Some(Value::Bool(*b));
    }
    if let Some(x) = field.try_downcast_ref::<f64>() {
        return Some(Value::Float(*x));
    }
    if let Some(x) = field.try_downcast_ref::<i64>() {
        return Some(Value::Int(*x));
    }
    if let Some(x) = field.try_downcast_ref::<i32>() {
        return Some(Value::Int(i64::from(*x)));
    }
    if let Some(x) = field.try_downcast_ref::<u32>() {
        return Some(Value::Int(i64::from(*x)));
    }
    if let Some(s) = field.try_downcast_ref::<String>() {
        return Some(Value::Text(s.clone()));
    }
    if let ReflectRef::Enum(e) = field.reflect_ref() {
        return Some(Value::Text(e.variant_name().to_string()));
    }
    None
}

/// Write `v` into a reflected field. Returns whether the value fitted the field.
pub fn set_value(field: &mut dyn PartialReflect, v: &Value) -> bool {
    match v {
        Value::Bool(b) => {
            if let Some(f) = field.try_downcast_mut::<bool>() {
                *f = *b;
                return true;
            }
        }
        Value::Float(x) => {
            if let Some(f) = field.try_downcast_mut::<f64>() {
                *f = *x;
                return true;
            }
        }
        Value::Int(x) => {
            if let Some(f) = field.try_downcast_mut::<i64>() {
                *f = *x;
                return true;
            }
            if let Some(f) = field.try_downcast_mut::<f64>() {
                *f = *x as f64;
                return true;
            }
        }
        Value::Text(s) => {
            if let Some(f) = field.try_downcast_mut::<String>() {
                f.clone_from(s);
                return true;
            }
            if let ReflectMut::Enum(e) = field.reflect_mut() {
                let dyn_enum = DynamicEnum::new(s.clone(), DynamicVariant::Unit);
                return e.try_apply(&dyn_enum).is_ok();
            }
        }
        _ => {}
    }
    false
}

/// Build a page for the reflected struct `T` from its inspector metadata. `prefix`: the
/// project-setting key prefix (`Some("editor")`), or `None` for user config.
pub fn page_for<T>(
    title: &str,
    defaults: &T,
    prefix: Option<&str>,
) -> Result<SettingsPage, EditorError>
where
    T: forge_reflect::ForgeApi + Struct + forge_reflect::bevy_reflect::TypePath,
{
    let mut reg = ForgeRegistry::new();
    reg.register::<T>()
        .map_err(|e| EditorError::Settings(e.to_string()))?;
    // Enum-typed fields list their variants only when the enum is registered too.
    let desc = reg.inspector(T::type_path()).ok_or_else(|| {
        EditorError::Settings(format!("{} is not a settings struct", T::type_path()))
    })?;
    let mut groups = Vec::new();
    for (cat, fields) in desc.groups {
        let mut rows = Vec::new();
        for f in fields {
            let Some(field) = defaults.field(f.name) else {
                continue;
            };
            let default = value_of(field).ok_or_else(|| {
                EditorError::Settings(format!("{}: unsupported field type", f.name))
            })?;
            let variants = match field.reflect_ref() {
                ReflectRef::Enum(_) => enum_variants(field),
                _ => Vec::new(),
            };
            rows.push(SettingRow {
                key: match prefix {
                    Some(p) => format!("{p}.{}", f.name),
                    None => f.name.to_string(),
                },
                field: f,
                default,
                variants,
            });
        }
        groups.push((cat.map(str::to_string), rows));
    }
    Ok(SettingsPage {
        title: title.to_string(),
        groups,
        project: prefix.is_some(),
    })
}

fn enum_variants(field: &dyn PartialReflect) -> Vec<String> {
    use forge_reflect::bevy_reflect::TypeInfo;
    match field.get_represented_type_info() {
        Some(TypeInfo::Enum(e)) => e.variant_names().iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// The editor settings page (user config).
pub fn editor_page() -> Result<SettingsPage, EditorError> {
    page_for(
        forge_ui::tr_key!("Editor"),
        &EditorSettings::default(),
        None,
    )
}

/// The project page for WP-U4's shared editor defaults.
pub fn project_editor_page() -> Result<SettingsPage, EditorError> {
    page_for(
        forge_ui::tr_key!("Grid and snapping"),
        &ProjectEditorSettings::default(),
        Some(PROJECT_EDITOR_PREFIX),
    )
}

impl EditorSettings {
    /// Set a field by name from a settings value (the Editor page's edits).
    pub fn set_field(&mut self, name: &str, v: &Value) -> bool {
        match self.field_mut(name) {
            Some(f) => set_value(f, v),
            None => false,
        }
    }

    /// Read a field by name as a settings value.
    pub fn get_field(&self, name: &str) -> Option<Value> {
        self.field(name).and_then(value_of)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_generated_from_reflection() {
        let p = editor_page().unwrap_or_else(|e| panic!("{e}"));
        let rows: Vec<(&str, RowEditor)> = p.rows().map(|r| (r.key.as_str(), r.editor())).collect();
        assert_eq!(
            rows,
            vec![
                ("theme", RowEditor::Choice),
                ("ui_scale", RowEditor::Slider),
                ("reduced_motion", RowEditor::Toggle),
                ("caret_blink", RowEditor::Toggle),
                ("first_run_tips", RowEditor::Toggle),
                ("gpu_mode", RowEditor::Choice),
            ]
        );
        let theme = p.rows().next().unwrap_or_else(|| panic!("row"));
        assert_eq!(theme.variants, vec!["Dark", "Light", "HighContrast"]);
        let scale = p.rows().nth(1).unwrap_or_else(|| panic!("row"));
        assert_eq!(scale.field.label, "UI scale");
        assert_eq!(scale.field.units, Some("%"));
        assert_eq!(scale.default, Value::Float(100.0));
        assert!(!p.project);

        let q = project_editor_page().unwrap_or_else(|e| panic!("{e}"));
        assert!(q.project);
        let keys: Vec<&str> = q.rows().map(|r| r.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "editor.grid_size",
                "editor.snap_translate",
                "editor.snap_rotate",
                "editor.up_axis",
                "editor.new_entity_name"
            ]
        );
        assert_eq!(q.rows().nth(4).map(|r| r.editor()), Some(RowEditor::Text));
    }

    #[test]
    fn editor_settings_fields_round_trip_through_reflection_and_ron() {
        let mut s = EditorSettings::default();
        assert!(s.set_field("theme", &Value::Text("Light".into())));
        assert_eq!(s.theme, EditorTheme::Light);
        assert!(!s.set_field("theme", &Value::Text("Nope".into())));
        assert!(s.set_field("ui_scale", &Value::Float(150.0)));
        assert!(s.set_field("caret_blink", &Value::Bool(false)));
        assert!(!s.set_field("caret_blink", &Value::Float(1.0)));
        assert_eq!(s.get_field("ui_scale"), Some(Value::Float(150.0)));
        let dir =
            std::env::temp_dir().join(format!("forge-editor-settings-{}", std::process::id()));
        s.save(&dir).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(EditorSettings::load(&dir).ok(), Some(s));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod gpu_mode_tests {
    use super::*;

    #[test]
    fn the_gpu_mode_preference_is_a_graphics_row_that_defaults_to_single() {
        assert_eq!(EditorSettings::default().gpu_mode, GpuModeSetting::Single);
        let p = editor_page().unwrap_or_else(|e| panic!("{e}"));
        let (group, rows) = p
            .groups
            .iter()
            .find(|(_, rows)| rows.iter().any(|r| r.key == "gpu_mode"))
            .unwrap_or_else(|| panic!("no gpu_mode row"));
        assert_eq!(group.as_deref(), Some("Graphics"));
        let row = &rows[0];
        assert_eq!(row.field.label, "GPU mode");
        assert_eq!(row.variants, vec!["Single", "Multi"]);
        assert_eq!(row.default, Value::Text("Single".into()));
        assert!(applies_at_next_start(&row.key));
        assert!(row_note(&row.key).is_some_and(|n| n.contains("next start")));
        assert!(
            variant_label(&row.key, "Multi").is_some_and(|l| l.contains("experimental")),
            "Multi is labelled experimental"
        );
        assert!(!applies_at_next_start("ui_scale"));
        assert_eq!(variant_label("theme", "Dark"), None);
    }

    #[test]
    fn the_gpu_mode_preference_round_trips_through_the_settings_file() {
        let mut s = EditorSettings::default();
        assert!(s.set_field("gpu_mode", &Value::Text("Multi".into())));
        assert_eq!(s.gpu_mode, GpuModeSetting::Multi);
        assert_eq!(s.get_field("gpu_mode"), Some(Value::Text("Multi".into())));
        let dir = std::env::temp_dir().join(format!("forge-gpu-mode-pref-{}", std::process::id()));
        s.save(&dir).unwrap_or_else(|e| panic!("{e}"));
        let back = EditorSettings::load(&dir).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back.gpu_mode, GpuModeSetting::Multi);
        // A settings file from before the preference existed loads as Single.
        std::fs::write(dir.join(EDITOR_SETTINGS_FILE), "(ui_scale: 120.0)")
            .unwrap_or_else(|e| panic!("{e}"));
        let old = EditorSettings::load(&dir).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(old.gpu_mode, GpuModeSetting::Single);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_project_graphics_page_is_project_state() {
        let p = project_graphics_page().unwrap_or_else(|e| panic!("{e}"));
        assert!(p.project);
        let keys: Vec<&str> = p.rows().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec![GPU_MODE_SETTING]);
        assert!(applies_at_next_start(GPU_MODE_SETTING));
        assert_eq!(GpuModeSetting::from_value(None), GpuModeSetting::Single);
        assert_eq!(
            GpuModeSetting::from_value(Some(&Value::Text("Multi".into()))),
            GpuModeSetting::Multi
        );
        assert_eq!(
            GpuModeSetting::from_value(Some(&Value::Text("multi".into()))),
            GpuModeSetting::Multi
        );
        assert_eq!(
            GpuModeSetting::from_value(Some(&Value::Bool(true))),
            GpuModeSetting::Single
        );
    }
}
