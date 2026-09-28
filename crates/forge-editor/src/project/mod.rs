//! The project lifecycle as the editor sees it (Ch.21 §21.21 "Launcher and new project",
//! "Preset and promotion", "Revision history", "Build and export"; Ch.31, Ch.33, Ch.38 §38.7;
//! WP-U7, ADR 0032).
//!
//! **Every flow is a command (I7).** Nothing here writes a file or a store:
//!
//! * **Session commands** — create, open, save, push, pull, build ([`ProjectOp`]) — are
//!   `Invoke`s the core registers ([`handlers`]). Their planners validate the arguments and
//!   change nothing; the bus audits them with their issuer; then the **core performs the
//!   store operation** (E-36) through `forge_project::host::ProjectHost` and reports an
//!   outcome in the `ProjectStatus` every client receives. An automation session, a script or
//!   `forge-editor --headless` saves and pushes exactly as the launcher and the history
//!   panel do.
//! * **Project edits** — promote to a preset ([`PROMOTE_CMD`]), link a remote
//!   ([`LINK_REMOTE_CMD`]) — are ordinary undoable commands planned on a `DiffBuilder`. A
//!   **lossy** promotion is refused unless the command says `"accept_loss": true`, and the
//!   refusal names what would be lost ([`promote`]): the confirmation the UI shows is also
//!   the core's rule, so an automation session cannot lose data by accident either.
//!
//! **Templates (O-14).** The new-project dialog offers 2D and 3D, then every preset a
//! plugin adds ([`Template::Preset`]; a plugin's template is a preset item too, carrying
//! its own defaults and new-scene template — I15 stays clean: presets set defaults and gate
//! nothing).

pub mod promote;
pub mod recent;

use std::collections::BTreeMap;

use forge_cmd::{CmdError, CommandPolicy, DiffBuilder, EditorCommand, Value};
use forge_project::format::{EntityDoc, ProjectDoc};
use forge_project::packager::Target;
use forge_project::{
    NAME_SETTING, PRESET_SETTING, ProjectError, TEMPLATE_SETTING, VERSION_SETTING,
};

pub use forge_project::format::LOAD_CMD;
pub use forge_project::status::{OpenProjectInfo, Outcome, ProjectStatus, RevisionInfo};
pub use forge_project::{REMOTE_SETTING, host::check_remote};

use crate::presets::builtin_preset;

/// Create a project (session command).
pub const CREATE_CMD: &str = "forge.project.create";
/// Open a project (session command).
pub const OPEN_CMD: &str = "forge.project.open";
/// Clone a project from a remote into a new folder (session command).
pub const CLONE_CMD: &str = "forge.project.clone";
/// Save: write and commit (session command).
pub const SAVE_CMD: &str = "forge.project.save";
/// Push to the linked remote (session command).
pub const PUSH_CMD: &str = "forge.project.push";
/// Pull from the linked remote (session command).
pub const PULL_CMD: &str = "forge.project.pull";
/// Build and export for a target (session command).
pub const BUILD_CMD: &str = "forge.export.build";
/// Link (or unlink) a remote: sets [`REMOTE_SETTING`] (an ordinary command).
pub const LINK_REMOTE_CMD: &str = "forge.project.link_remote";
/// Promote the project to another preset (an ordinary command).
pub const PROMOTE_CMD: &str = "forge.project.promote";
/// Sign in to GitHub with the device flow: start, or continue once the code is entered
/// (session command, WP-16). The token never crosses the bus: the core keeps it.
pub const SIGN_IN_CMD: &str = "forge.project.sign_in";
/// Create a private GitHub repository for the project and link it as the remote (session
/// command, WP-16; Ch.33.5 "repo created for you").
pub const CREATE_REMOTE_CMD: &str = "forge.project.create_remote";

/// The session commands the core performs.
pub const SESSION_TARGETS: &[&str] = &[
    CREATE_CMD,
    OPEN_CMD,
    CLONE_CMD,
    SAVE_CMD,
    PUSH_CMD,
    PULL_CMD,
    BUILD_CMD,
    SIGN_IN_CMD,
    CREATE_REMOTE_CMD,
];

/// Whether `target` is one of the lifecycle session commands.
pub fn is_session_target(target: &str) -> bool {
    SESSION_TARGETS.contains(&target)
}

/// A lifecycle command's title for toasts and the history panel ("Save", "Push").
pub fn op_title(target: &str) -> &'static str {
    match target {
        CREATE_CMD => "Create project",
        OPEN_CMD => "Open project",
        CLONE_CMD => "Clone project",
        SAVE_CMD => "Save",
        PUSH_CMD => "Push",
        PULL_CMD => "Pull",
        BUILD_CMD => "Build",
        SIGN_IN_CMD => "Sign in",
        CREATE_REMOTE_CMD => "Create remote",
        _ => "Project",
    }
}

/// The save command (`message` empty: the revision is named by its summary).
pub fn save_command(message: &str) -> EditorCommand {
    ProjectOp::Save {
        message: message.to_string(),
    }
    .command()
}

// ---- templates --------------------------------------------------------------------------

/// A new-project template (O-14), or a preset a plugin provides (WP-21).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Template {
    TwoD,
    ThreeD,
    /// A preset of the editor's `Preset` registry by key (a preset plugin's, source or WASM):
    /// the new-project flow lists them after the built-in templates. Its id is
    /// `preset:<key>`; the core creates it from its [`crate::presets::PresetCatalog`].
    Preset(String),
}

/// The id prefix of a registry preset ([`Template::Preset`]).
pub const PRESET_TEMPLATE_PREFIX: &str = "preset:";

impl Template {
    /// In the order the new-project dialog lists them.
    pub const ALL: [Template; 2] = [Template::TwoD, Template::ThreeD];
    /// Its id (`3d`, `preset:com.example.rivers`).
    pub fn id(&self) -> String {
        match self {
            Template::TwoD => "2d".into(),
            Template::ThreeD => "3d".into(),
            Template::Preset(k) => format!("{PRESET_TEMPLATE_PREFIX}{k}"),
        }
    }
    /// Its label (a registry preset's key: the dialog shows the catalog's label).
    pub fn label(&self) -> &str {
        match self {
            Template::TwoD => "2D",
            Template::ThreeD => "3D",
            Template::Preset(k) => k,
        }
    }
    /// One line for the dialog.
    pub fn blurb(&self) -> &'static str {
        match self {
            Template::TwoD => {
                "Sprites, tile maps and 2D physics on the 2D render path. Loads no 3D plugin."
            }
            Template::ThreeD => "One world frame, static meshes, perspective camera.",
            Template::Preset(_) => {
                "A preset a plugin provides: its family's defaults with its own."
            }
        }
    }
    /// The built-in workspace preset it uses (a directory under `presets/`); `None` for a
    /// registry preset (its family says which, in the catalog).
    pub fn preset(&self) -> Option<&'static str> {
        match self {
            Template::TwoD => Some("2d"),
            Template::ThreeD => Some("3d"),
            Template::Preset(_) => None,
        }
    }
    pub fn from_id(id: &str) -> Option<Template> {
        if let Some(key) = id.strip_prefix(PRESET_TEMPLATE_PREFIX) {
            return forge_plugin::check_key(key)
                .ok()
                .map(|()| Template::Preset(key.to_string()));
        }
        Self::ALL.into_iter().find(|t| t.id() == id)
    }
}

/// A preset manifest's setting value (RON text: `1`, `4.5`, `true`, `"3d"`) as a `Value`.
pub fn ron_value(text: &str) -> Option<Value> {
    let t = text.trim();
    match t {
        "true" => return Some(Value::Bool(true)),
        "false" => return Some(Value::Bool(false)),
        _ => {}
    }
    if t.starts_with('"') {
        return ron::from_str::<String>(t).ok().map(Value::Text);
    }
    if let Ok(i) = t.parse::<i64>() {
        return Some(Value::Int(i));
    }
    t.parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .map(Value::Float)
}

/// A preset's default project settings (from its `workspace.ron`). A value that does not
/// read is skipped (the preset loader already checked the manifest).
pub fn preset_defaults(preset: &str) -> Result<BTreeMap<String, Value>, ProjectError> {
    let p = builtin_preset(preset).map_err(|e| ProjectError::BadFiles(e.to_string()))?;
    Ok(defaults_of(&p))
}

/// A loaded preset's default project settings as values.
pub fn defaults_of(p: &crate::presets::Preset) -> BTreeMap<String, Value> {
    p.workspace
        .settings
        .iter()
        .filter_map(|(k, v)| ron_value(v).map(|v| (k.clone(), v)))
        .collect()
}

/// The document a new project from `template` starts as: the preset's default settings,
/// the name, and the preset's new-scene template (data: `presets/<preset>/templates/`,
/// Ch.31 §31.3).
pub fn template_doc(template: Template, name: &str) -> Result<ProjectDoc, ProjectError> {
    template_doc_in(&template, name, &crate::presets::PresetCatalog::builtin())
}

/// [`template_doc`] from the editor's preset catalog (its real `Preset` registry): a
/// built-in template uses what its family's key holds there (a plugin may have replaced or
/// chained it), and a [`Template::Preset`] is created from that registry item — refused,
/// naming it, when no loaded plugin provides it.
pub fn template_doc_in(
    template: &Template,
    name: &str,
    catalog: &crate::presets::PresetCatalog,
) -> Result<ProjectDoc, ProjectError> {
    let bad = |e: crate::EditorError| ProjectError::BadFiles(e.to_string());
    let (preset, dir) = match template {
        Template::Preset(key) => {
            let e = catalog.get(key).ok_or_else(|| {
                ProjectError::BadFiles(format!(
                    "no loaded plugin provides the preset {key:?} (is its plugin installed and enabled?)"
                ))
            })?;
            (e.preset.clone(), e.dir())
        }
        other => {
            let dir = other.preset().unwrap_or("3d");
            (catalog.family(dir).map_err(bad)?, dir.to_string())
        }
    };
    let mut settings = defaults_of(&preset);
    let t = |s: &str| Value::Text(s.to_string());
    settings.insert(NAME_SETTING.into(), t(name));
    settings.insert(PRESET_SETTING.into(), t(&dir));
    settings.insert(TEMPLATE_SETTING.into(), t(&template.id()));
    settings.insert(VERSION_SETTING.into(), t("0.1.0"));
    let entities: Vec<EntityDoc> = preset.scene.clone();
    Ok(ProjectDoc { settings, entities })
}

// ---- lifecycle operations (session commands) ---------------------------------------------

/// A lifecycle operation the core performs (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectOp {
    /// Create a project at `location` (`file:<folder>`, `memory:<name>`) from `template`.
    /// `discard_unsaved`: the open project's unsaved changes may be dropped (the user
    /// confirmed); otherwise they refuse the operation.
    Create {
        location: String,
        name: String,
        template: Template,
        discard_unsaved: bool,
    },
    Open {
        location: String,
        discard_unsaved: bool,
    },
    /// Clone the project at `remote` into a new project at `location` (every revision,
    /// ids intact); the remote stays linked.
    Clone {
        remote: String,
        location: String,
        discard_unsaved: bool,
    },
    /// `message` empty: the revision's summary.
    Save {
        message: String,
    },
    Push,
    Pull,
    Build {
        target: Target,
    },
    /// Sign in to GitHub: start the device flow (`resume` false), or ask once whether the
    /// user entered the code (`resume` true).
    SignIn {
        resume: bool,
    },
    /// Create a private GitHub repository named `name` and link it as the remote.
    CreateRemote {
        name: String,
    },
}

impl ProjectOp {
    /// The command target.
    pub fn target(&self) -> &'static str {
        match self {
            ProjectOp::Create { .. } => CREATE_CMD,
            ProjectOp::Open { .. } => OPEN_CMD,
            ProjectOp::Clone { .. } => CLONE_CMD,
            ProjectOp::Save { .. } => SAVE_CMD,
            ProjectOp::Push => PUSH_CMD,
            ProjectOp::Pull => PULL_CMD,
            ProjectOp::Build { .. } => BUILD_CMD,
            ProjectOp::SignIn { .. } => SIGN_IN_CMD,
            ProjectOp::CreateRemote { .. } => CREATE_REMOTE_CMD,
        }
    }

    /// The bus command.
    pub fn command(&self) -> EditorCommand {
        let args = match self {
            ProjectOp::Create {
                location,
                name,
                template,
                discard_unsaved,
            } => serde_json::json!({
                "location": location,
                "name": name,
                "template": template.id(),
                "discard_unsaved": discard_unsaved,
            }),
            ProjectOp::Open {
                location,
                discard_unsaved,
            } => serde_json::json!({ "location": location, "discard_unsaved": discard_unsaved }),
            ProjectOp::Clone {
                remote,
                location,
                discard_unsaved,
            } => serde_json::json!({
                "remote": remote,
                "location": location,
                "discard_unsaved": discard_unsaved,
            }),
            ProjectOp::Save { message } => serde_json::json!({ "message": message }),
            ProjectOp::Push | ProjectOp::Pull => serde_json::json!({}),
            ProjectOp::Build { target } => serde_json::json!({ "target": target.id() }),
            ProjectOp::SignIn { resume } => {
                serde_json::json!({ "provider": "github", "resume": resume })
            }
            ProjectOp::CreateRemote { name } => {
                serde_json::json!({ "provider": "github", "name": name })
            }
        };
        EditorCommand::Invoke {
            target: self.target().into(),
            args: args.to_string(),
        }
    }
}

fn arg_str<'a>(args: &'a serde_json::Value, k: &str) -> Result<&'a str, String> {
    args.get(k)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("\"{k}\" must be text"))
}

fn arg_bool(args: &serde_json::Value, k: &str) -> Result<bool, String> {
    match args.get(k) {
        None => Ok(false),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| format!("\"{k}\" must be true or false")),
    }
}

/// A project location: `file:<folder>`, `git:<folder>` (a folder whose history is a Git
/// repository) or `memory:<name>`.
fn check_location(loc: &str) -> Result<(), String> {
    match loc
        .strip_prefix("file:")
        .or_else(|| loc.strip_prefix("git:"))
        .or_else(|| loc.strip_prefix("memory:"))
    {
        Some(rest) if !rest.trim().is_empty() => Ok(()),
        _ => Err(format!(
            "{loc:?} is not a project location: use file:<folder> or git:<folder> (or \
             memory:<name>)"
        )),
    }
}

fn check_provider(args: &serde_json::Value) -> Result<(), String> {
    match args.get("provider").and_then(serde_json::Value::as_str) {
        Some("github") | None => Ok(()),
        Some(p) => Err(format!(
            "unknown provider {p:?}: GitHub (\"github\") is the one built"
        )),
    }
}

/// Read a lifecycle command's arguments (the planner's check and the core's decoding are
/// this one function, so what the bus accepts is exactly what the core runs).
pub fn parse_op(target: &str, args: &serde_json::Value) -> Result<ProjectOp, String> {
    Ok(match target {
        CREATE_CMD => {
            let location = arg_str(args, "location")?.trim().to_string();
            check_location(&location)?;
            let name = arg_str(args, "name")?.trim().to_string();
            if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
                return Err("\"name\" must be 1-128 characters with no control characters".into());
            }
            let t = arg_str(args, "template")?;
            let template = Template::from_id(t).ok_or_else(|| {
                format!("unknown template {t:?}: use 2d, 3d or preset:<key> (a plugin's preset)")
            })?;
            ProjectOp::Create {
                location,
                name,
                template,
                discard_unsaved: arg_bool(args, "discard_unsaved")?,
            }
        }
        OPEN_CMD => {
            let location = arg_str(args, "location")?.trim().to_string();
            check_location(&location)?;
            ProjectOp::Open {
                location,
                discard_unsaved: arg_bool(args, "discard_unsaved")?,
            }
        }
        CLONE_CMD => {
            let remote = arg_str(args, "remote")?.trim().to_string();
            check_remote(&remote).map_err(|e| e.to_string())?;
            let location = arg_str(args, "location")?.trim().to_string();
            check_location(&location)?;
            ProjectOp::Clone {
                remote,
                location,
                discard_unsaved: arg_bool(args, "discard_unsaved")?,
            }
        }
        SAVE_CMD => ProjectOp::Save {
            message: match args.get("message") {
                None => String::new(),
                Some(_) => arg_str(args, "message")?.to_string(),
            },
        },
        PUSH_CMD => ProjectOp::Push,
        PULL_CMD => ProjectOp::Pull,
        BUILD_CMD => {
            let t = arg_str(args, "target")?;
            ProjectOp::Build {
                target: Target::from_id(t).ok_or_else(|| {
                    format!("unknown target {t:?}: use windows, linux, web or android")
                })?,
            }
        }
        SIGN_IN_CMD => {
            check_provider(args)?;
            ProjectOp::SignIn {
                resume: arg_bool(args, "resume")?,
            }
        }
        CREATE_REMOTE_CMD => {
            check_provider(args)?;
            let name = arg_str(args, "name")?.trim().to_string();
            let ok = !name.is_empty()
                && name.len() <= 100
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                && !name.starts_with('.');
            if !ok {
                return Err(format!(
                    "{name:?} is not a repository name: 1-100 of A-Z a-z 0-9 - _ . (not \
                     starting with a dot)"
                ));
            }
            ProjectOp::CreateRemote { name }
        }
        other => return Err(format!("{other:?} is not a project lifecycle command")),
    })
}

/// Decode a lifecycle session command the core accepted.
pub fn decode_op(target: &str, args: &str) -> Option<Result<ProjectOp, String>> {
    is_session_target(target).then(|| {
        serde_json::from_str::<serde_json::Value>(args)
            .map_err(|e| e.to_string())
            .and_then(|v| parse_op(target, &v))
    })
}

fn bad(target: &str, why: impl Into<String>) -> CmdError {
    CmdError::BadArgs {
        target: target.to_string(),
        why: why.into(),
    }
}

/// The link-a-remote command (`url` empty: unlink).
pub fn link_remote_command(url: &str) -> EditorCommand {
    EditorCommand::Invoke {
        target: LINK_REMOTE_CMD.into(),
        args: serde_json::json!({ "url": url }).to_string(),
    }
}

fn plan_link_remote(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let url = arg_str(args, "url")
        .map_err(|w| bad(LINK_REMOTE_CMD, w))?
        .trim()
        .to_string();
    if url.is_empty() {
        return b.set_setting(REMOTE_SETTING, None);
    }
    check_remote(&url).map_err(|e| bad(LINK_REMOTE_CMD, e.to_string()))?;
    b.set_setting(REMOTE_SETTING, Some(Value::Text(url)))
}

/// A command planner.
pub type Planner = fn(&mut DiffBuilder<'_>, &serde_json::Value) -> Result<(), CmdError>;

macro_rules! session_planner {
    ($name:ident, $target:expr) => {
        fn $name(_b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
            // A session command: validated, and no change to the project, ever.
            parse_op($target, args)
                .map(drop)
                .map_err(|w| bad($target, w))
        }
    };
}
session_planner!(plan_create, CREATE_CMD);
session_planner!(plan_open, OPEN_CMD);
session_planner!(plan_clone, CLONE_CMD);
session_planner!(plan_save, SAVE_CMD);
session_planner!(plan_push, PUSH_CMD);
session_planner!(plan_pull, PULL_CMD);
session_planner!(plan_build, BUILD_CMD);
session_planner!(plan_sign_in, SIGN_IN_CMD);
session_planner!(plan_create_remote, CREATE_REMOTE_CMD);

/// The project load's policy: never undone or redone. It replaces the whole project, security
/// state included, so an undo or a redo of one could grant (Ch.21 §21.18); the core clears
/// the history after a load anyway.
pub const LOAD_POLICY: CommandPolicy = CommandPolicy {
    human_only: false,
    undoable: false,
    redoable: false,
};

/// The lifecycle commands' planners, for the core to register (only the core names the
/// bus, I7's split rule): `(target, policy, planner)`.
pub fn handlers() -> Vec<(&'static str, CommandPolicy, Planner)> {
    vec![
        (CREATE_CMD, CommandPolicy::ORDINARY, plan_create),
        (OPEN_CMD, CommandPolicy::ORDINARY, plan_open),
        (CLONE_CMD, CommandPolicy::ORDINARY, plan_clone),
        (SAVE_CMD, CommandPolicy::ORDINARY, plan_save),
        (PUSH_CMD, CommandPolicy::ORDINARY, plan_push),
        (PULL_CMD, CommandPolicy::ORDINARY, plan_pull),
        (BUILD_CMD, CommandPolicy::ORDINARY, plan_build),
        (SIGN_IN_CMD, CommandPolicy::ORDINARY, plan_sign_in),
        (
            CREATE_REMOTE_CMD,
            CommandPolicy::ORDINARY,
            plan_create_remote,
        ),
        (LINK_REMOTE_CMD, CommandPolicy::ORDINARY, plan_link_remote),
        (PROMOTE_CMD, CommandPolicy::ORDINARY, promote::plan_promote),
        (LOAD_CMD, LOAD_POLICY, crate::security::plan_load),
    ]
}

/// A default location for a new project named `name` under `dir`: `file:<dir>/<ident>`.
pub fn default_location(dir: &std::path::Path, name: &str) -> String {
    format!(
        "file:{}",
        dir.join(crate::domain::ident_from(name)).display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_carry_their_preset_defaults() {
        for t in Template::ALL {
            let d = template_doc(t.clone(), "Demo").unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(
                d.settings.get(PRESET_SETTING),
                t.preset().map(|p| Value::Text(p.into())).as_ref()
            );
            assert!(d.settings.contains_key("frames.depth"), "{t:?}");
            assert!(!d.entities.is_empty());
        }
        // A registry preset no loaded plugin provides is refused, naming it.
        let e = template_doc_in(
            &Template::Preset("com.example.none".into()),
            "x",
            &crate::presets::PresetCatalog::builtin(),
        )
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
        assert!(e.contains("com.example.none"), "{e}");
        assert_eq!(ron_value("\"3d\""), Some(Value::Text("3d".into())));
        assert_eq!(ron_value("8"), Some(Value::Int(8)));
        assert_eq!(ron_value("true"), Some(Value::Bool(true)));
    }

    #[test]
    fn lifecycle_arguments_round_trip_and_bad_ones_are_named() {
        let ops = [
            ProjectOp::Create {
                location: "file:C:/p/demo".into(),
                name: "Demo".into(),
                template: Template::Preset("com.example.rivers".into()),
                discard_unsaved: false,
            },
            ProjectOp::Open {
                location: "memory:x".into(),
                discard_unsaved: true,
            },
            ProjectOp::Clone {
                remote: "file:D:/nas/x".into(),
                location: "memory:y".into(),
                discard_unsaved: false,
            },
            ProjectOp::Save {
                message: "first".into(),
            },
            ProjectOp::Push,
            ProjectOp::Pull,
            ProjectOp::Build {
                target: Target::Linux,
            },
        ];
        for op in ops {
            let EditorCommand::Invoke { target, args } = op.command() else {
                panic!("not an invoke");
            };
            assert_eq!(decode_op(&target, &args), Some(Ok(op)));
        }
        let e = |t: &str, a: serde_json::Value| parse_op(t, &a).err().unwrap_or_default();
        assert!(
            e(
                CREATE_CMD,
                serde_json::json!({"location": "C:/x", "name": "a", "template": "3d"})
            )
            .contains("file:")
        );
        assert!(
            e(
                CREATE_CMD,
                serde_json::json!({"location": "file:x", "name": "", "template": "3d"})
            )
            .contains("name")
        );
        assert!(
            e(
                CREATE_CMD,
                serde_json::json!({"location": "file:x", "name": "a", "template": "4d"})
            )
            .contains("template")
        );
        assert!(e(BUILD_CMD, serde_json::json!({"target": "psp"})).contains("target"));
        assert!(decode_op(PROMOTE_CMD, "{}").is_none());
    }
}
