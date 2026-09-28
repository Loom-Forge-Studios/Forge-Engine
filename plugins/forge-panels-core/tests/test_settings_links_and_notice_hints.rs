//! `test_settings_links_and_notice_hints` (WP-47, gate `C-plugin-settings-link-and-notice-hint`):
//! **a plugin adds a link beside a settings row and a hint to the problems it knows, through the
//! base `SettingLink` and `NoticeHint` points — the base editor names none of them.**
//!
//! * A test plugin's `SettingLink` for the Project page's default-automation-capabilities row
//!   shows a button beside that row, labelled with the link's (looked-up) text; pressing it opens
//!   the plugin's panel and changes nothing in the project.
//! * The editor's own dialogs come first: a plugin's link for a row the editor already opens a
//!   dialog for (`project.preset`) does not replace that dialog.
//! * A test plugin's `NoticeHint` for a code prefix and for one exact code: a problem posted with
//!   a generic next step (looked up from its code, or from a refusal) gets the hint's next step
//!   and a button opening the hint's panel; an exact code beats a prefix; a next step the poster
//!   wrote for the problem is kept (only the button is added); a problem no hint matches is
//!   untouched.
//!
//! Positive control (W2): `positive_control_an_editor_without_the_plugin_offers_neither` — the
//! same checks on the same editor without the plugin fail, naming the row with no link and the
//! problem with no hint, so the checks cannot pass on the base editor's own behaviour.

mod common;

use forge_editor::notify::{
    HintMatch, NoticeAction, NoticeHint, NoticeHintPoint, Problem, Severity,
};
use forge_editor::settings::{SettingLink, SettingLinkPoint};
use forge_editor::testing::Rig;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

const SETTINGS: &str = "forge.settings";
const ROW: &str = "project.automation_policy";
const PANEL: &str = "test.helper";

/// A plugin with one panel, a link beside [`ROW`] (and one beside `project.preset`, which the
/// editor must keep for its own dialog), and two hints.
struct Helper {
    manifest: Manifest,
}

impl Helper {
    fn new() -> Self {
        let text = format!(
            "Plugin(id: \"test.helper\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, \
             provides: [EditorPanel(\"{PANEL}\"), SettingLink(\"{ROW}\"), \
             SettingLink(\"project.preset\"), NoticeHint(\"test.hint.prefix\"), \
             NoticeHint(\"test.hint.exact\")])"
        );
        Self {
            manifest: Manifest::parse(&text).unwrap_or_else(|e| panic!("{e}")),
        }
    }
}

impl SourcePlugin for Helper {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.add::<EditorPanel<forge_editor::panels::PanelCx>>(
            PANEL,
            PanelDescriptor {
                title: "Helper".into(),
                icon: None,
                default_dock: Dock::Right,
                build: std::sync::Arc::new(|cx: &mut forge_editor::panels::PanelCx| {
                    cx.never_empty("a test panel");
                }),
            },
            Order::Last,
        )?;
        cx.add::<SettingLinkPoint>(
            ROW,
            SettingLink {
                panel: PANEL.into(),
                label: "Manage\u{2026}".into(),
            },
            Order::Last,
        )?;
        cx.add::<SettingLinkPoint>(
            "project.preset",
            SettingLink {
                panel: PANEL.into(),
                label: "Hijack".into(),
            },
            Order::Last,
        )?;
        cx.add::<NoticeHintPoint>(
            "test.hint.prefix",
            NoticeHint {
                applies_to: HintMatch::Prefix("TEST".into()),
                next: Some("Fix it in Helper.".into()),
                open: Some((PANEL.into(), "Helper".into())),
            },
            Order::Last,
        )?;
        cx.add::<NoticeHintPoint>(
            "test.hint.exact",
            NoticeHint {
                applies_to: HintMatch::Code("CMD-0014".into()),
                next: Some("Ask in Helper.".into()),
                open: Some((PANEL.into(), "Helper".into())),
            },
            Order::Last,
        )?;
        Ok(())
    }
}

fn rig(extra: &[&dyn SourcePlugin]) -> Rig {
    let mut rig = Rig::new(common::config(extra, None)).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(&[SETTINGS])
        .unwrap_or_else(|e| panic!("{e}"));
    let _ = rig.h.ui.a11y_activate();
    rig.settle();
    rig
}

/// The Project page row's link: its label and whether pressing it opens [`PANEL`] and nothing
/// else. `Err` names what is missing.
fn check_link(rig: &mut Rig) -> Result<(), String> {
    let line = common::part(rig, SETTINGS, &["content", &format!("lifecycle.{ROW}")]);
    let open = line.child(&forge_ui::Key::Str("open".into()));
    if !rig.h.ui.contains(open) {
        return Err(format!("{ROW}: no link beside the row"));
    }
    let label = rig
        .h
        .ui
        .a11y_node(open)
        .and_then(|n| n.label().map(str::to_string))
        .unwrap_or_default();
    if label != "Manage\u{2026}" {
        return Err(format!("{ROW}: the link reads {label:?}"));
    }
    let h = rig.state_hash();
    rig.h.ui.raise(open, forge_ui::widgets::Pressed(open));
    rig.turn();
    rig.settle();
    if rig.state_hash() != h {
        return Err(format!("{ROW}: following the link changed the project"));
    }
    if !rig
        .shell
        .layout()
        .panels()
        .iter()
        .any(|p| p.as_str() == PANEL)
    {
        return Err(format!(
            "{ROW}: the link did not open {PANEL}: {:?}",
            rig.shell.layout().panels()
        ));
    }
    Ok(())
}

/// The last notification's next step and whether it offers [`PANEL`].
fn last(rig: &Rig) -> (String, bool) {
    let s = rig.shell.session();
    let n = s
        .notifications
        .history()
        .last()
        .unwrap_or_else(|| panic!("nothing was posted"));
    let offers = n
        .actions
        .iter()
        .any(|a| matches!(a, NoticeAction::OpenPanel { panel, .. } if panel == PANEL));
    (n.next.clone().unwrap_or_default(), offers)
}

/// The hints, applied as the module docs say. `Err` names the first problem that is wrong.
fn check_hints(rig: &mut Rig) -> Result<(), String> {
    let post = |rig: &mut Rig, p: Problem| {
        rig.shell.session_mut().problem(p);
    };
    // A generic next step for a code under the prefix.
    post(
        rig,
        Problem::coded(Severity::Error, "TEST-0001", "Refused", "why"),
    );
    if last(rig) != ("Fix it in Helper.".into(), true) {
        return Err(format!("TEST-0001 (prefix, generic): {:?}", last(rig)));
    }
    // A refusal by policy: the exact code's hint, not a prefix's.
    let r = forge_cmd::Rejection::new(
        forge_cmd::CmdError::PolicyRefused {
            what: "grant".into(),
        },
        None,
        None,
    );
    post(rig, Problem::from_rejection("Grant refused", &r));
    if last(rig) != ("Ask in Helper.".into(), true) {
        return Err(format!("CMD-0014 (exact, from a refusal): {:?}", last(rig)));
    }
    // A next step the poster wrote is kept; the button is still offered.
    post(
        rig,
        Problem::error("TEST-0002", "Refused", "why", "Do the specific thing."),
    );
    if last(rig) != ("Do the specific thing.".into(), true) {
        return Err(format!("TEST-0002 (poster's own step): {:?}", last(rig)));
    }
    // No hint: untouched.
    post(
        rig,
        Problem::coded(Severity::Warning, "ASSET-0002", "Import failed", "why"),
    );
    let (next, offers) = last(rig);
    if offers || next != forge_editor::notify::next_step_for_code("ASSET-0002") {
        return Err(format!(
            "ASSET-0002 (no hint) was changed: {next:?} {offers}"
        ));
    }
    if rig.shell.session().notifications.incomplete() != 0 {
        return Err("a hinted problem was counted incomplete".into());
    }
    Ok(())
}

#[test]
fn a_plugin_links_a_settings_row_to_its_panel() {
    let helper = Helper::new();
    let mut rig = rig(&[&helper]);
    check_link(&mut rig).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn the_editors_own_dialog_comes_before_a_plugins_link() {
    let helper = Helper::new();
    let rig = rig(&[&helper]);
    let open = common::part(
        &rig,
        SETTINGS,
        &["content", "lifecycle.project.preset", "open"],
    );
    let label = rig
        .h
        .ui
        .a11y_node(open)
        .and_then(|n| n.label().map(str::to_string))
        .unwrap_or_default();
    assert_eq!(
        label, "Change preset\u{2026}",
        "the plugin replaced the editor's dialog"
    );
    // The seam's own resolution: base first, then the plugin's link.
    let links = &rig.shell.handles().services().setting_links;
    assert_eq!(
        forge_editor::settings::row_link("project.preset", links).map(|l| l.0),
        Some("forge.presets".to_string())
    );
    assert_eq!(
        forge_editor::settings::row_link(ROW, links).map(|l| l.0),
        Some(PANEL.to_string())
    );
}

#[test]
fn a_plugins_hints_give_problems_its_next_step_and_panel() {
    let helper = Helper::new();
    let mut rig = rig(&[&helper]);
    assert_eq!(rig.shell.session().notifications.hints().len(), 2);
    check_hints(&mut rig).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_editor_without_the_plugin_offers_neither() {
    let mut rig = rig(&[]);
    let e = check_link(&mut rig).expect_err("a row with no plugin link passed");
    assert!(e.contains("no link beside the row"), "{e}");
    let e = check_hints(&mut rig).expect_err("a problem with no plugin hint passed");
    assert!(e.contains("TEST-0001"), "{e}");
}
