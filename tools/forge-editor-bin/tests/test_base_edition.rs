//! `test_base_edition` (ADR 0061, WP-42; gate row `C-base-edition-no-session-host`): **the
//! base editor has no automation entry point** — no session host, no flag that starts one,
//! no panel for one. The premium edition adds all of it from its own crates through the
//! program's edition hooks (`forge_editor_bin::app::EditorEdition`); the base binary runs
//! with [`BaseEdition`], which adds nothing. (That no base crate links a premium crate at all
//! is `cargo xtask premium-boundary`'s P1.)
//!
//! * `the_base_edition_adds_nothing` — `BaseEdition` has no flags, no plugins and no usage
//!   text; every argument it is asked about is not its own.
//! * `the_base_editor_accepts_only_the_base_flags` — the real `forge-editor` binary's
//!   `--help` names exactly the base flags ([`BASE_FLAGS`]: the split-editor host is the one
//!   listener), and a flag outside them is refused before anything starts (no window, no
//!   listener: the process exits with the usage error).
//! * `the_base_editor_offers_no_session_panel` — the editor the binary assembles from its
//!   first-party plugins offers no panel whose id or title is about automation sessions, and
//!   its services carry no plugin-added service.
//!
//! Positive controls (W2):
//! * `positive_control_an_edition_with_a_flag_is_seen` — an edition adding a listener flag
//!   makes the usage check fail, naming it;
//! * `positive_control_a_session_panel_is_seen` — a plugin adding an "Agent sessions" panel
//!   to the first-party set makes the panel check fail, naming it.

use std::collections::BTreeSet;
use std::process::Command;

use forge_editor::panels::PanelCx;
use forge_editor::presets::builtin_preset;
use forge_editor::shell::assemble;
use forge_editor_bin::app::{BaseEdition, EditorEdition, usage};
use forge_editor_bin::first_party::FirstParty;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

/// Every flag the base editor takes (its usage names exactly these).
const BASE_FLAGS: &[&str] = &[
    "--config-dir",
    "--connect",
    "--device-name",
    "--exit-after",
    "--headless",
    "--lan",
    "--no-remote-host",
    "--no-user-config",
    "--pair",
    "--play-2d-sample",
    "--port",
    "--preset",
    "--remote-host",
    "--report-startup",
    "--script",
    "--accept-project-security",
    "--trust-project",
];

/// The `--flags` a usage text names.
fn flags_in(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|w| w.starts_with("--") && w.len() > 2)
        .map(str::to_string)
        .collect()
}

/// The usage names exactly the base flags.
fn check_usage(text: &str) -> Result<(), String> {
    let want: BTreeSet<String> = BASE_FLAGS.iter().map(|s| (*s).to_string()).collect();
    let got = flags_in(text);
    let extra: Vec<&String> = got.difference(&want).collect();
    let missing: Vec<&String> = want.difference(&got).collect();
    if extra.is_empty() && missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the usage names flags beyond the base editor's: {extra:?} (missing: {missing:?})"
        ))
    }
}

/// Panels whose id or title is about automation sessions.
fn session_panels(plugins: &[&dyn SourcePlugin]) -> Result<Vec<String>, String> {
    let preset = builtin_preset("3d").map_err(|e| e.to_string())?;
    let cfg = assemble(preset, plugins, &[], None).map_err(|e| e.to_string())?;
    if !cfg.services.extensions.is_empty() {
        return Err(format!(
            "the base editor's services carry {} plugin-added service(s)",
            cfg.services.extensions.len()
        ));
    }
    Ok(cfg
        .panels
        .iter()
        .filter(|(id, d)| {
            let text = format!("{id} {}", d.title).to_lowercase();
            ["agent", "automation", "session host"]
                .iter()
                .any(|w| text.contains(w))
        })
        .map(|(id, d)| format!("{id} ({})", d.title))
        .collect())
}

fn check_panels(plugins: &[&dyn SourcePlugin]) -> Result<(), String> {
    let found = session_panels(plugins)?;
    if found.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the base editor offers session panels: {}",
            found.join(", ")
        ))
    }
}

#[test]
fn the_base_edition_adds_nothing() {
    let mut e = BaseEdition;
    assert!(e.usage().is_empty());
    assert!(e.plugins().is_empty());
    let args: Vec<String> = ["--listen", "127.0.0.1:0", "--demo"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    for i in 0..args.len() {
        assert!(e.parse_arg(&args, i).is_none(), "{} was taken", args[i]);
    }
    assert!(e.check_args(true).is_ok() && e.check_args(false).is_ok());
    check_usage(&usage(&e)).unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn the_base_editor_accepts_only_the_base_flags() {
    let exe = env!("CARGO_BIN_EXE_forge-editor");
    let help = Command::new(exe)
        .arg("--help")
        .output()
        .unwrap_or_else(|e| panic!("{exe}: {e}"));
    let text = String::from_utf8_lossy(&help.stderr).into_owned();
    check_usage(&text).unwrap_or_else(|err| panic!("{err}\n{text}"));
    // A flag the base does not know is refused at once: nothing starts (no window, no
    // listener), the usage error says why.
    let out = Command::new(exe)
        .args([
            "--no-user-config",
            "--no-remote-host",
            "--listen",
            "127.0.0.1:0",
        ])
        .output()
        .unwrap_or_else(|e| panic!("{exe}: {e}"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "an unknown flag was accepted: {err}");
    assert!(err.contains("unknown argument --listen"), "{err}");
    assert!(
        out.stdout.is_empty(),
        "the refused run printed: {:?}",
        out.stdout
    );
}

#[test]
fn the_base_editor_offers_no_session_panel() {
    let fp = FirstParty::new().unwrap_or_else(|e| panic!("{e}"));
    check_panels(&fp.plugins()).unwrap_or_else(|e| panic!("{e}"));
}

/// An edition that adds one listener flag (the control's).
struct Listening;

impl EditorEdition for Listening {
    fn usage(&self) -> &'static str {
        "       forge-editor [...] [--listen-for-sessions ADDR]"
    }
}

#[test]
fn positive_control_an_edition_with_a_flag_is_seen() {
    let e = check_usage(&usage(&Listening)).expect_err("an extra flag must fail the check");
    assert!(e.contains("--listen-for-sessions"), "{e}");
}

/// A plugin with one panel about automation sessions (the control's).
struct SessionsPanel {
    manifest: Manifest,
}

impl SessionsPanel {
    fn new() -> Result<Self, PluginError> {
        Ok(Self {
            manifest: Manifest::parse(
                "Plugin(id: \"com.example.sessions\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [EditorPanel(\"com.example.agent_sessions\")])",
            )?,
        })
    }
}

impl SourcePlugin for SessionsPanel {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.add::<EditorPanel<PanelCx>>(
            "com.example.agent_sessions",
            PanelDescriptor {
                title: "Agent sessions".into(),
                icon: None,
                default_dock: Dock::Right,
                build: std::sync::Arc::new(|_: &mut PanelCx| {}),
            },
            Order::Last,
        )?;
        Ok(())
    }
}

#[test]
fn positive_control_a_session_panel_is_seen() {
    let fp = FirstParty::new().unwrap_or_else(|e| panic!("{e}"));
    let extra = SessionsPanel::new().unwrap_or_else(|e| panic!("{e}"));
    let mut plugins = fp.plugins();
    plugins.push(&extra);
    let e = check_panels(&plugins).expect_err("a session panel must fail the check");
    assert!(e.contains("com.example.agent_sessions"), "{e}");
}
