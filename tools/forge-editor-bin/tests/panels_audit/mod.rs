//! The editor-wide panel audits (WP-U12; Ch.21 §21.21 "Requirements on every panel", §21.10,
//! §21.8, §21.9; DoD M2-70, M2-31, M2-22's audit), shared by every edition's test (WP-47): the
//! base editor's `tests/test_editor_panels_audit.rs` runs them over the first-party plugin set,
//! and an edition that adds panels runs **the same audits** over the first-party set plus its
//! own plugins ([`Edition`]), so a panel an edition brings is held to exactly the rules the
//! base panels are — no forked copy that drifts.
//!
//! Each panel is opened alone in the headless editor loop. The screen-reader, contrast,
//! keyboard and pseudo-locale audits run **twice**: on an empty project in an editor with no
//! services, and on a populated project in the editor as the binary wires it
//! (`forge_editor_bin::wiring`: asset database, connect services, in-memory team server and
//! licence, then the edition's own services, [`Edition::attach`]) with a user config
//! directory, so every first-run tip shows (`populated`, `populate`).
//!
//! * Empty states: every panel shows its empty state from the catalogue widget (`EmptyState`:
//!   text and at most one action) — or it declared that it always has content
//!   (`PanelCx::never_empty`) and shows some.
//! * First-run tips: every panel has a tip; it shows on first open (with a user config
//!   directory), takes no focus, dismisses to nothing — **0 frames and 0 wakeups** over 10 s
//!   after — and stays dismissed.
//! * Screen-reader tree: every visible focusable widget of every panel has a role, a non-empty
//!   accessible name and the Focus action in the AccessKit tree.
//! * Contrast: every `(fg, bg)` pair the panels paint, in all three themes, meets its theme's
//!   floor (§21.8).
//! * Keyboard-only walkthrough (M2-31): Tab from the panel's first control visits every
//!   visible focusable widget of the panel and comes back, each stop named.
//! * Pseudo-locale (M2-31): every visible string is a localisation key.
//! * Idle with every panel open: 0 frames and 0 wakeups.
//!
//! Every audit first checks **coverage**: the panels the edition names in
//! [`Edition::must_include`] are among those audited ([`coverage_problems`]), so an edition's
//! audit cannot pass by not loading its panels. The positive controls (a panel breaking each
//! rule) are the `control_*` functions.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use forge_editor::core::SharedCore;
use forge_editor::panels::{EditorPanelHost, PanelCatalog, PanelCx};
use forge_editor::presets::builtin_preset;
use forge_editor::services::PanelFaults;
use forge_editor::settings::EditorTheme;
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::testing::Rig;
use forge_editor_bin::first_party::FirstParty;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};
use forge_ui::dock::PanelId;
use forge_ui::style::ColorPair;
use forge_ui::widgets::{Button, Container, EmptyState, IconButton};
use forge_ui::{FocusScope, Key, NodeStyle, Role, Theme, WidgetId};

// ---- the edition under audit ----------------------------------------------------------------

/// What the edition's populated editor keeps alive (its services' backends, open sessions).
pub type Keep = Box<dyn std::any::Any>;

/// The edition whose panels are audited: the first-party plugin set plus `plugins`.
pub struct Edition<'a> {
    /// A short name (temporary directories, messages).
    pub name: &'static str,
    /// The plugins the edition loads beside the first-party set.
    pub plugins: Vec<&'a dyn SourcePlugin>,
    /// Attach the edition's services to the populated editor, after the binary's wiring (as
    /// its `EditorEdition::attach` does); what it returns lives as long as the editor.
    pub attach: fn(&mut ShellConfig, &SharedCore) -> Option<Keep>,
    /// Panels that must be among those audited (an edition's own panels).
    pub must_include: Vec<&'static str>,
    /// The fewest panels the editor must have.
    pub min_panels: usize,
    /// Where the keyboard walkthrough record is written with `FORGE_WRITE_EVIDENCE=1`,
    /// relative to the repository root (`None`: not written).
    pub evidence: Option<&'static str>,
    /// Data the edition's own panels show, like [`PANEL_DATA`] (allowed in that panel only).
    pub panel_data: &'static [PanelData],
}

/// A panel, the data words it shows, and why they are data.
pub type PanelData = (&'static str, &'static [&'static str], &'static str);

fn no_attach(_: &mut ShellConfig, _: &SharedCore) -> Option<Keep> {
    None
}

impl Edition<'static> {
    /// The base editor: the first-party plugins, nothing else.
    #[must_use]
    pub fn base() -> Self {
        Self {
            name: "base",
            plugins: Vec::new(),
            attach: no_attach,
            must_include: Vec::new(),
            // The base editor's own panel catalogue (plugins add theirs on top).
            min_panels: 34,
            evidence: Some("docs/evidence/keyboard-walkthrough.md"),
            panel_data: &[],
        }
    }
}

/// The panels of [`Edition::must_include`] missing from `panels` (empty: covered).
#[must_use]
pub fn coverage_problems(ed: &Edition, panels: &[String]) -> Vec<String> {
    ed.must_include
        .iter()
        .filter(|p| !panels.iter().any(|q| q == *p))
        .map(|p| format!("{}: {p} is not among the audited panels", ed.name))
        .collect()
}

fn assert_covers(ed: &Edition, e: &Editor) {
    let missing = coverage_problems(ed, &e.panels);
    assert!(missing.is_empty(), "{}", missing.join("\n"));
    assert!(
        e.panels.len() >= ed.min_panels,
        "{}: only {} panels",
        ed.name,
        e.panels.len()
    );
}

// ---- the editor under audit ---------------------------------------------------------------

/// What a panel declared about itself.
#[derive(Clone, Debug, Default)]
struct Decl {
    never_empty: Option<String>,
    tip: Option<String>,
}

pub struct Editor {
    pub rig: Rig,
    pub panels: Vec<String>,
    decls: BTreeMap<String, Decl>,
    /// The populated pass: the first-run tips show and are audited with their panel.
    tips: bool,
    /// A user config directory this editor owns (removed when it is dropped).
    config_dir: Option<PathBuf>,
    /// What the populated pass put in the project.
    pop: Option<Populated>,
    /// What the edition's services keep alive.
    _keep: Option<Keep>,
}

impl Drop for Editor {
    fn drop(&mut self) {
        if let Some(d) = &self.config_dir {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

fn config(ed: &Edition, extra: &[&dyn SourcePlugin], config_dir: Option<PathBuf>) -> ShellConfig {
    let fp = FirstParty::new().unwrap_or_else(|e| panic!("{e}"));
    let mut all = fp.plugins();
    all.extend_from_slice(&ed.plugins);
    all.extend_from_slice(extra);
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    assemble(preset, &all, &[], config_dir).unwrap_or_else(|e| panic!("{e}"))
}

fn declarations(cfg: &ShellConfig) -> (Vec<String>, BTreeMap<String, Decl>) {
    let kind = cfg.preset.workspace.family.kind();
    let host = EditorPanelHost::new(&cfg.panels, kind, PanelCatalog::default());
    let panels: Vec<String> = cfg.panels.iter().map(|(k, _)| k.to_string()).collect();
    let decls = panels
        .iter()
        .map(|p| {
            let cx = host.describe(&PanelId::new(p));
            let d = cx.map_or_else(Decl::default, |cx| Decl {
                never_empty: cx.never_empty_reason().map(str::to_string),
                tip: cx.tip().map(str::to_string),
            });
            (p.clone(), d)
        })
        .collect();
    (panels, decls)
}

fn editor_with(cfg: ShellConfig) -> Editor {
    let (panels, decls) = declarations(&cfg);
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    let _ = rig.h.ui.a11y_activate();
    Editor {
        rig,
        panels,
        decls,
        tips: false,
        config_dir: None,
        pop: None,
        _keep: None,
    }
}

/// The edition's editor with `extra` plugins, no services, no user config.
pub fn editor(ed: &Edition, extra: &[&dyn SourcePlugin]) -> Editor {
    editor_with(config(ed, extra, None))
}

// ---- the populated pass --------------------------------------------------------------------

/// Now, as the binary's clock gives it: the in-memory Team licence is issued today and
/// valid for a year, so collaboration is on.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The editor as the binary wires it — the asset database, the connect services, the
/// labelled in-memory team server with a Team licence pinned (so the collaboration panels show
/// their content), the collaboration panels' services (`forge_editor_bin::wiring`, the code
/// `main` runs), then the edition's own services — with a user config directory, so every
/// panel shows its first-run tip, over a populated project ([`populate`]). Only what needs a
/// network or a GPU is left out: the remote-host listener, the pool's adapters in the compute
/// panel.
pub fn populated(ed: &Edition, extra: &[&dyn SourcePlugin], tag: &str) -> Editor {
    let dir = temp_config(&format!("{}-{tag}", ed.name));
    let mut cfg = config(ed, extra, Some(dir.clone()));
    let (panels, decls) = declarations(&cfg);
    let core = forge_editor::core::EditorCore::new();
    cfg.services.set_assets(
        forge_editor_bin::wiring::asset_catalog(&cfg).unwrap_or_else(|e| panic!("{e}")),
    );
    cfg.services.connect.attach(&core, Some(&dir));
    let baseline = format!(
        "panels-audit-{}-{tag}-{}-{:?}",
        ed.name,
        std::process::id(),
        std::thread::current().id()
    );
    forge_editor_bin::wiring::attach_team_server(&core, "tester".into(), &baseline, now_ms());
    cfg.services.collab.attach(&core);
    let keep = (ed.attach)(&mut cfg, &core);
    let mut rig = Rig::with_core(cfg, core).unwrap_or_else(|e| panic!("{e}"));
    let _ = rig.h.ui.a11y_activate();
    let mut e = Editor {
        rig,
        panels,
        decls,
        tips: true,
        config_dir: Some(dir),
        pop: None,
        _keep: keep,
    };
    e.pop = Some(populate(&mut e));
    e
}

/// What the populated project holds.
struct Populated {
    /// A parent entity with a child; the child carries every built-in component and is
    /// selected (the inspector shows every editor kind).
    parent: forge_cmd::EntityKey,
    child: forge_cmd::EntityKey,
    /// Asset paths imported into a folder.
    assets: Vec<String>,
}

/// Fill the project the way a person would, through the bus as `human:tester`: entities
/// with every built-in component, one selected, an image imported into a folder, a team
/// created. Names are identifiers (`ground_01`), so what the pseudo-locale audit reads as
/// words is only ever UI text.
fn populate(e: &mut Editor) -> Populated {
    use forge_cmd::{EditorCommand, Issuer};
    use forge_editor::client::BusClient;
    let key = |e: &Editor| forge_editor::core::EditorCore::read(&e.rig.core, |p| p.next_key());
    let mut bus = e.rig.connect(Issuer::Human {
        user: "tester".into(),
    });
    let parent = key(e);
    bus.apply(
        EditorCommand::Spawn {
            name: "ground_01".into(),
            parent: None,
        },
        None,
    );
    let _ = bus.pump();
    let child = key(e);
    bus.apply(
        EditorCommand::Spawn {
            name: "crate_01".into(),
            parent: Some(parent),
        },
        None,
    );
    let _ = bus.pump();
    let cmds: Vec<EditorCommand> = e
        .rig
        .shell
        .handles()
        .services()
        .components
        .components()
        .flat_map(|c| c.add_commands(child))
        .collect();
    for c in cmds {
        bus.apply(c, None);
    }
    let _ = bus.pump();
    // An image dropped into a folder, imported by command (what the asset browser does).
    let png = temp_png();
    let catalog = e
        .rig
        .shell
        .handles()
        .services()
        .assets
        .clone()
        .unwrap_or_else(|| panic!("the binary wires an asset database"));
    let staged = catalog
        .borrow_mut()
        .stage(&[png], "textures_01")
        .unwrap_or_else(|x| panic!("{x}"));
    let assets: Vec<String> = staged
        .into_iter()
        .filter(|p| catalog.borrow().importable(p))
        .collect();
    for a in &assets {
        bus.apply(forge_editor::assets::import_command(a), None);
    }
    let _ = bus.pump();
    bus.apply(forge_editor::collab::create_command("team_01"), None);
    let _ = bus.pump();
    e.rig.settle();
    e.rig.shell.session_mut().set_selection(vec![child]);
    e.rig.settle();
    Populated {
        parent,
        child,
        assets,
    }
}

/// The populated pass is not vacuous: the project holds what [`populate`] put in it, and
/// the panels the empty pass could only see as their empty state show their content — the
/// asset browser its grid of the imported image, the inspector the selected entity's
/// components, the team panel the team, every panel its first-run tip.
pub fn populated_pass_reach(ed: &Edition) {
    let mut e = populated(ed, &[], "reach");
    assert_covers(ed, &e);
    let pop = e.pop.as_ref().unwrap_or_else(|| panic!("populated"));
    let (parent, child) = (pop.parent, pop.child);
    assert!(!pop.assets.is_empty(), "the PNG was not importable");
    let entities = forge_editor::core::EditorCore::read(&e.rig.core, |p| {
        [parent, child]
            .iter()
            .filter(|k| p.entity(**k).is_some())
            .count()
    });
    assert_eq!(entities, 2, "the two entities exist");
    let has_team = forge_editor::core::EditorCore::collab_status(&e.rig.core)
        .is_some_and(|s| s.team.is_some());
    assert!(has_team, "the team was created");
    let named = |e: &mut Editor, panel: &str| -> usize {
        let f = open(e, panel).unwrap_or_else(|x| panic!("{x}"));
        visible(e, f)
            .into_iter()
            .filter(|i| e.rig.h.ui.is_focusable(*i))
            .count()
    };
    // The edition's own panels too: its services were attached, so they show content.
    for p in [
        "forge.assets",
        "forge.inspector",
        "forge.team",
        "forge.licence",
        "forge.profiler",
    ]
    .into_iter()
    .chain(ed.must_include.iter().copied())
    {
        assert!(
            named(&mut e, p) > 0,
            "{p}: nothing to reach on a populated project"
        );
    }
    let f = open(&mut e, "forge.assets").unwrap_or_else(|x| panic!("{x}"));
    assert!(
        visible(&e, f)
            .iter()
            .any(|i| e.rig.h.ui.widget::<EmptyState>(*i).is_none()
                && e.rig
                    .h
                    .ui
                    .widget::<forge_ui::widgets::VirtualGrid>(*i)
                    .is_some()),
        "the asset browser shows its grid"
    );
    let tips = e
        .panels
        .clone()
        .into_iter()
        .filter(|p| {
            open(&mut e, p).ok().is_some_and(|f| {
                e.rig
                    .h
                    .ui
                    .is_visible(f.child(&Key::Static("first_run_tip")))
            })
        })
        .count();
    assert_eq!(
        tips,
        e.panels.len(),
        "every panel shows its tip in the populated pass"
    );
}

/// A 16 x 16 checker PNG in a fresh temp directory.
fn temp_png() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "forge-panels-audit-png-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::create_dir_all(&d);
    let p = d.join("bricks_01.png");
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u32 {
        for x in 0..16u32 {
            let on = (x / 4 + y / 4) % 2 == 0;
            rgba.extend_from_slice(if on {
                &[200, 60, 40, 255]
            } else {
                &[40, 40, 200, 255]
            });
        }
    }
    let f = std::fs::File::create(&p).unwrap_or_else(|x| panic!("{x}"));
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), 16, 16);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap_or_else(|x| panic!("{x}"));
    w.write_image_data(&rgba).unwrap_or_else(|x| panic!("{x}"));
    p
}

/// A fresh user config directory (tips need one to remember a dismissal).
fn temp_config(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "forge-panels-audit-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    d
}

fn subtree(ui: &forge_ui::Ui, root: WidgetId) -> Vec<WidgetId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        out.push(id);
        let mut c = ui.children(id);
        c.reverse();
        stack.extend(c);
    }
    out
}

/// Open `panel` alone; its frame.
fn open(e: &mut Editor, panel: &str) -> Result<WidgetId, String> {
    e.rig
        .show_panels(&[panel])
        .map_err(|x| format!("{panel}: {x}"))?;
    let f = e
        .rig
        .panel_frame(panel)
        .ok_or_else(|| format!("{panel}: no frame"))?;
    if e.rig.h.ui.contains(f.child(&Key::Static("missing"))) {
        return Err(format!("{panel}: did not build (placeholder shown)"));
    }
    Ok(f)
}

/// The visible widgets of `panel`'s frame; the tip bar is left out of the empty pass (whose
/// editor has no user config, so no tip shows) and is part of the populated pass.
fn visible(e: &Editor, frame: WidgetId) -> Vec<WidgetId> {
    let tip = frame.child(&Key::Static("first_run_tip"));
    let skip: BTreeSet<WidgetId> = if !e.tips && e.rig.h.ui.contains(tip) {
        subtree(&e.rig.h.ui, tip).into_iter().collect()
    } else {
        BTreeSet::new()
    };
    subtree(&e.rig.h.ui, frame)
        .into_iter()
        .filter(|i| *i != frame && !skip.contains(i) && e.rig.h.ui.is_visible(*i))
        .collect()
}

fn a11y_text(e: &Editor, id: WidgetId) -> Option<String> {
    let n = e.rig.h.ui.a11y_node(id)?;
    n.label()
        .or_else(|| n.value())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ---- test panels (the positive controls) ---------------------------------------------------

/// A test panel: its id and build function.
type TestPanel = (&'static str, fn(&mut PanelCx));

struct TestPanels {
    manifest: Manifest,
    panels: Vec<TestPanel>,
}

impl TestPanels {
    fn new(panels: Vec<TestPanel>) -> Self {
        let provides: Vec<String> = panels
            .iter()
            .map(|(id, _)| format!("EditorPanel(\"{id}\")"))
            .collect();
        let text = format!(
            "Plugin(id: \"test.audit\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Self {
            manifest: Manifest::parse(&text).unwrap_or_else(|e| panic!("{e}")),
            panels,
        }
    }
}

impl SourcePlugin for TestPanels {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for (id, build) in &self.panels {
            cx.add::<EditorPanel<PanelCx>>(
                id,
                PanelDescriptor {
                    title: (*id).to_string(),
                    icon: None,
                    default_dock: Dock::Center,
                    build: std::sync::Arc::new(*build),
                },
                Order::Last,
            )?;
        }
        Ok(())
    }
}

fn blank_panel(cx: &mut PanelCx) {
    cx.add(|b, parent| {
        b.add(
            parent,
            "body",
            NodeStyle::leaf().grow(1.0),
            Container::group(),
        )?;
        Ok(())
    });
}

fn never_empty_but_blank(cx: &mut PanelCx) {
    cx.never_empty("it always shows its list");
    blank_panel(cx);
}

fn unlabelled_icon_panel(cx: &mut PanelCx) {
    cx.never_empty("a toolbar");
    cx.add(|b, parent| {
        b.add(parent, "ok", NodeStyle::leaf(), Button::new("Labelled"))?;
        b.add(
            parent,
            "bad",
            NodeStyle::leaf(),
            IconButton::unlabelled("\u{2605}"),
        )?;
        Ok(())
    });
}

fn trapping_panel(cx: &mut PanelCx) {
    cx.never_empty("two buttons");
    cx.add(|b, parent| {
        let inner = b.add(
            parent,
            "inner",
            NodeStyle::row(4.0).scope(FocusScope::Panel),
            Container::group().labelled("Trap"),
        )?;
        b.add(inner, "a", NodeStyle::leaf(), Button::new("Inside"))?;
        b.add(parent, "b", NodeStyle::leaf(), Button::new("Outside"))?;
        Ok(())
    });
}

// ---- empty states (M2-70) ------------------------------------------------------------------

/// Every panel's empty-state problems (empty: all good), and how many panels showed their
/// catalogue empty state.
fn empty_state_problems(e: &mut Editor) -> (Vec<String>, usize) {
    let mut problems = Vec::new();
    let mut shown = 0;
    for p in e.panels.clone() {
        let frame = match open(e, &p) {
            Ok(f) => f,
            Err(x) => {
                problems.push(x);
                continue;
            }
        };
        let vis = visible(e, frame);
        let empties: Vec<WidgetId> = vis
            .iter()
            .copied()
            .filter(|i| e.rig.h.ui.widget::<EmptyState>(*i).is_some())
            .collect();
        if let Some(es) = empties.first() {
            let msg = e
                .rig
                .h
                .ui
                .widget::<EmptyState>(*es)
                .map(|w| w.message(e.rig.h.ui.rt()))
                .unwrap_or_default();
            if msg.trim().is_empty() {
                problems.push(format!("{p}: its empty state has no text"));
            }
            shown += 1;
            continue;
        }
        let content = vis.iter().filter(|i| a11y_text(e, **i).is_some()).count();
        match &e.decls[&p].never_empty {
            Some(why) if content > 0 => {
                let _ = why;
            }
            Some(why) => problems.push(format!(
                "{p}: declares it always has content ({why}) but shows a blank area"
            )),
            None => problems.push(format!(
                "{p}: shows no empty state from the catalogue widget on an empty project, and \
                 does not declare that it always has content ({content} labelled widgets)"
            )),
        }
    }
    (problems, shown)
}

pub fn empty_states(ed: &Edition) {
    let mut e = editor(ed, &[]);
    assert_covers(ed, &e);
    let (problems, shown) = empty_state_problems(&mut e);
    assert!(
        problems.is_empty(),
        "M2-70 empty states:\n{}",
        problems.join("\n")
    );
    assert!(shown >= 20, "only {shown} panels showed an empty state");
    println!(
        "empty states ({}): {} panels, {shown} show their empty state on an empty project",
        ed.name,
        e.panels.len()
    );
}

pub fn control_blank_panel(ed: &Edition) {
    let t = TestPanels::new(vec![
        ("test.blank", blank_panel),
        ("test.never_empty_but_blank", never_empty_but_blank),
    ]);
    let mut e = editor(ed, &[&t]);
    let (problems, _) = empty_state_problems(&mut e);
    assert!(
        problems.iter().any(|p| p.starts_with("test.blank:")),
        "a blank panel passed: {problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("test.never_empty_but_blank:")),
        "a never-empty panel showing nothing passed: {problems:?}"
    );
}

// ---- first-run tips (M2-70) ----------------------------------------------------------------

/// Every panel's tip, checked (see the module docs); `Err` names the first panel that breaks
/// a rule.
fn check_tips(ed: &Edition, faults: PanelFaults, only: Option<&str>) -> Result<usize, String> {
    let dir = temp_config(&format!("{}-tips", ed.name));
    let mut cfg = config(ed, &[], Some(dir.clone()));
    cfg.services.faults = faults;
    let mut e = editor_with(cfg);
    if only.is_none() {
        let missing = coverage_problems(ed, &e.panels);
        if !missing.is_empty() {
            return Err(missing.join("\n"));
        }
    }
    let mut checked = 0;
    for p in e.panels.clone() {
        if only.is_some_and(|o| o != p) {
            continue;
        }
        let tip_text = e.decls[&p]
            .tip
            .clone()
            .ok_or_else(|| format!("{p}: declares no first-run tip"))?;
        if tip_text.trim().is_empty() {
            return Err(format!("{p}: an empty tip"));
        }
        let focus_before = e.rig.h.ui.focused();
        let frame = open(&mut e, &p)?;
        let bar = frame.child(&Key::Static("first_run_tip"));
        if !e.rig.h.ui.is_visible(bar) {
            return Err(format!("{p}: its tip does not show on first open"));
        }
        let focus = e.rig.h.ui.focused();
        if focus.is_some_and(|f| subtree(&e.rig.h.ui, bar).contains(&f)) && focus != focus_before {
            return Err(format!("{p}: its tip took the focus"));
        }
        e.rig.h.click(bar.child(&Key::Static("dismiss")));
        e.rig.settle();
        if e.rig.h.ui.contains(bar) && e.rig.h.ui.is_visible(bar) {
            return Err(format!("{p}: the tip is still shown after dismissal"));
        }
        // The dismissal is user config: the shell's debounced settings save wakes the loop
        // once to write it (no frame). After that, nothing at all.
        let (frames, _save) = e.rig.advance(Duration::from_secs(10));
        let (later_frames, wakeups) = e.rig.advance(Duration::from_secs(10));
        if (frames, later_frames, wakeups) != (0, 0, 0) {
            return Err(format!(
                "{p}: a dismissed tip left work behind: {frames} frames in the first 10 s, then \
                 {later_frames} frames and {wakeups} wakeups in the next 10 s"
            ));
        }
        if !e.rig.shell.session().settings().tip_seen(&p) {
            return Err(format!("{p}: the dismissal is not kept in user config"));
        }
        // Reopened: no tip.
        e.rig
            .show_panels(&["forge.console"])
            .map_err(|x| x.to_string())?;
        let frame = open(&mut e, &p)?;
        if e.rig
            .h
            .ui
            .contains(frame.child(&Key::Static("first_run_tip")))
        {
            return Err(format!("{p}: the tip came back after it was dismissed"));
        }
        checked += 1;
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(checked)
}

/// The panel a control that breaks one panel breaks: the edition's own first panel, else
/// `fallback` (a base panel).
fn control_panel<'a>(ed: &Edition, fallback: &'a str) -> &'a str {
    ed.must_include.first().copied().unwrap_or(fallback)
}

pub fn tips(ed: &Edition) {
    let n = check_tips(ed, PanelFaults::default(), None).unwrap_or_else(|e| panic!("{e}"));
    assert!(n >= ed.min_panels, "only {n} tips checked");
    println!(
        "first-run tips ({}): {n} panels, each dismissed to 0 frames / 0 wakeups over 10 s",
        ed.name
    );
}

pub fn control_tip_timer(ed: &Edition) {
    let e = check_tips(
        ed,
        PanelFaults {
            tip_keeps_timer: true,
            ..PanelFaults::default()
        },
        Some(control_panel(ed, "forge.inspector")),
    )
    .expect_err("a tip that keeps animating after dismissal passed");
    assert!(e.contains("left work behind"), "{e}");
}

pub fn no_tips_without_config(ed: &Edition) {
    let mut e = editor(ed, &[]);
    let f = open(&mut e, control_panel(ed, "forge.hierarchy")).unwrap_or_else(|x| panic!("{x}"));
    assert!(
        !e.rig.h.ui.contains(f.child(&Key::Static("first_run_tip"))),
        "no user config directory: a dismissal could not be remembered, so no tip"
    );
    let dir = temp_config(&format!("{}-tips-off", ed.name));
    forge_editor::settings::EditorSettings {
        first_run_tips: false,
        ..Default::default()
    }
    .save(&dir)
    .unwrap_or_else(|x| panic!("{x}"));
    let mut e = editor_with(config(ed, &[], Some(dir.clone())));
    let f = open(&mut e, control_panel(ed, "forge.inspector")).unwrap_or_else(|x| panic!("{x}"));
    assert!(
        !e.rig.h.ui.contains(f.child(&Key::Static("first_run_tip"))),
        "tips turned off in Settings"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- screen-reader tree (AccessKit) --------------------------------------------------------

fn a11y_problems(e: &mut Editor) -> (Vec<String>, usize) {
    let mut problems = Vec::new();
    let mut checked = 0;
    for p in e.panels.clone() {
        let frame = match open(e, &p) {
            Ok(f) => f,
            Err(x) => {
                problems.push(x);
                continue;
            }
        };
        for id in visible(e, frame) {
            if !e.rig.h.ui.is_focusable(id) {
                continue;
            }
            let Some(node) = e.rig.h.ui.a11y_node(id) else {
                problems.push(format!("{p}: focusable {id:?} has no a11y node"));
                continue;
            };
            if node.role() == Role::Unknown {
                problems.push(format!("{p}: {id:?} has no role"));
            }
            let name = node.label().unwrap_or("").trim().to_string();
            if name.is_empty() {
                problems.push(format!(
                    "{p}: {id:?} ({:?}) has no accessible name",
                    node.role()
                ));
            }
            if !node.supports_action(accesskit::Action::Focus) {
                problems.push(format!("{p}: {id:?} is focusable but has no Focus action"));
            }
            checked += 1;
        }
    }
    (problems, checked)
}

pub fn a11y(ed: &Edition) {
    let mut e = editor(ed, &[]);
    assert_covers(ed, &e);
    let (problems, checked) = a11y_problems(&mut e);
    assert!(
        problems.is_empty(),
        "screen-reader tree (empty project):\n{}",
        problems.join("\n")
    );
    assert!(checked >= 200, "only {checked} widgets audited");
    let mut p = populated(ed, &[], "a11y");
    let (problems, populated_checked) = a11y_problems(&mut p);
    assert!(
        problems.is_empty(),
        "screen-reader tree (populated project):\n{}",
        problems.join("\n")
    );
    assert!(
        populated_checked > checked,
        "the populated pass reached no more widgets ({populated_checked}) than the empty one ({checked})"
    );
    println!(
        "screen-reader tree ({}): {checked} focusable widgets on an empty project and \
         {populated_checked} on a populated one, across {} panels, each with a role, a name and Focus",
        ed.name,
        e.panels.len()
    );
}

pub fn control_unlabelled(ed: &Edition) {
    let t = TestPanels::new(vec![("test.unlabelled", unlabelled_icon_panel)]);
    let mut e = editor(ed, &[&t]);
    let (problems, _) = a11y_problems(&mut e);
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("test.unlabelled:") && p.contains("no accessible name")),
        "{problems:?}"
    );
}

// ---- contrast (§21.8), all three themes ----------------------------------------------------

fn contrast_failures(theme: &Theme, pairs: &BTreeMap<ColorPair, String>) -> Vec<String> {
    pairs
        .iter()
        .filter_map(|(p, panel)| {
            let c = theme.contrast(*p);
            let floor = theme.floor(p.kind);
            (c + 1e-3 < floor).then(|| {
                format!(
                    "{} ({panel}): {} on {} ({:?}) = {c:.2}:1 < {floor}:1",
                    theme.name,
                    p.fg.token(),
                    p.bg.token(),
                    p.kind
                )
            })
        })
        .collect()
}

/// Paint every panel in `theme` (on an empty project, or a populated one with its tips);
/// the pairs used, each with the first panel that painted it.
fn panel_pairs(ed: &Edition, theme: EditorTheme, full: bool) -> BTreeMap<ColorPair, String> {
    let mut by: BTreeMap<ColorPair, String> = BTreeMap::new();
    let mut e = if full {
        populated(ed, &[], &format!("contrast-{theme:?}"))
    } else {
        editor(ed, &[])
    };
    assert_covers(ed, &e);
    e.rig
        .shell
        .session_mut()
        .update_settings(|s| s.theme = theme);
    e.rig.settle();
    for p in e.panels.clone() {
        if let Ok(f) = open(&mut e, &p) {
            // Keyboard focus on the panel's controls paints the focus ring pairs too.
            let ids: Vec<WidgetId> = visible(&e, f)
                .into_iter()
                .filter(|i| e.rig.h.ui.is_focusable(*i))
                .take(6)
                .collect();
            for id in ids {
                e.rig.h.ui.set_focus(Some(id), true);
                e.rig.settle();
            }
        }
        for pair in e.rig.h.ui.used_pairs() {
            by.entry(*pair).or_insert_with(|| p.clone());
        }
    }
    by
}

pub fn contrast(ed: &Edition) {
    let mut total = 0;
    for t in [
        EditorTheme::Dark,
        EditorTheme::Light,
        EditorTheme::HighContrast,
    ] {
        let mut pairs = panel_pairs(ed, t, false);
        for (pair, panel) in panel_pairs(ed, t, true) {
            pairs.entry(pair).or_insert(panel);
        }
        assert!(pairs.len() >= 15, "{t:?}: only {} pairs", pairs.len());
        let f = contrast_failures(&t.theme(), &pairs);
        assert!(f.is_empty(), "contrast failures:\n{}", f.join("\n"));
        total += pairs.len();
    }
    println!(
        "contrast ({}): {total} (fg, bg) pairs across the three themes, all at or above their \
         floors",
        ed.name
    );
}

pub fn control_low_contrast(ed: &Edition) {
    let pairs = panel_pairs(ed, EditorTheme::Dark, false);
    let mut theme = Theme::dark();
    theme.set_color(
        forge_ui::ColorRole::FgMuted,
        forge_ui::Color::from_rgba8(0x3a, 0x3c, 0x42, 255),
    );
    let f = contrast_failures(&theme, &pairs);
    assert!(f.iter().any(|l| l.contains("fg.muted on")), "{f:?}");
}

// ---- keyboard-only walkthrough (M2-31) -----------------------------------------------------

/// One panel's walk: every Tab stop's role and name, in order; `Err` if a visible focusable
/// widget is never reached or a stop has no name.
fn walk(e: &mut Editor, p: &str) -> Result<Vec<String>, String> {
    let frame = open(e, p)?;
    let vis = visible(e, frame);
    let focusables: Vec<WidgetId> = vis
        .iter()
        .copied()
        .filter(|i| e.rig.h.ui.is_focusable(*i))
        .collect();
    let Some(first) = focusables.first().copied() else {
        return Ok(Vec::new());
    };
    e.rig.h.ui.set_focus(Some(first), true);
    e.rig.settle();
    let mut seen = BTreeSet::new();
    let mut stops = Vec::new();
    for _ in 0..(focusables.len() * 2 + 8) {
        let Some(f) = e.rig.h.ui.focused() else { break };
        if !seen.insert(f) {
            break;
        }
        let name = e
            .rig
            .h
            .ui
            .a11y_node(f)
            .and_then(|n| n.label().map(str::to_string))
            .unwrap_or_default();
        let role = e
            .rig
            .h
            .ui
            .a11y_node(f)
            .map_or_else(|| "?".to_string(), |n| format!("{:?}", n.role()));
        if name.trim().is_empty() {
            return Err(format!("{p}: a Tab stop ({role}) has no name"));
        }
        stops.push(format!("{role} \u{201c}{name}\u{201d}"));
        e.rig.h.tab(false);
        e.rig.settle();
    }
    let missed: Vec<WidgetId> = focusables
        .iter()
        .copied()
        .filter(|f| !seen.contains(f))
        .collect();
    if !missed.is_empty() {
        return Err(format!(
            "{p}: {} of {} focusable widgets are never reached by Tab",
            missed.len(),
            focusables.len()
        ));
    }
    Ok(stops)
}

/// Walk every panel of `e`; `(stops per panel, problems)`.
fn walk_all(e: &mut Editor) -> (BTreeMap<String, Vec<String>>, Vec<String>) {
    let mut stops = BTreeMap::new();
    let mut problems = Vec::new();
    for p in e.panels.clone() {
        match walk(e, &p) {
            Ok(s) => {
                stops.insert(p, s);
            }
            Err(x) => problems.push(x),
        }
    }
    (stops, problems)
}

fn record_pass(record: &mut String, title: &str, stops: &BTreeMap<String, Vec<String>>) {
    let total: usize = stops.values().map(Vec::len).sum();
    record.push_str(&format!("# {title} \u{2014} {total} stops\n\n"));
    for (p, s) in stops {
        record.push_str(&format!("## {p} \u{2014} {} stops\n\n", s.len()));
        for (i, stop) in s.iter().enumerate() {
            record.push_str(&format!("{}. {stop}\n", i + 1));
        }
        record.push('\n');
    }
}

pub fn walkthrough(ed: &Edition) {
    let mut e = editor(ed, &[]);
    assert_covers(ed, &e);
    let (empty, mut problems) = walk_all(&mut e);
    let mut p = populated(ed, &[], "walk");
    let (full, more) = walk_all(&mut p);
    problems.extend(more);
    // Every panel can be entered from the keyboard once the project has content: a panel
    // with no Tab stop there is one a keyboard user cannot use.
    for (panel, s) in &full {
        if s.is_empty() {
            problems.push(format!(
                "{panel}: no Tab stop on a populated project (the keyboard cannot enter it)"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "keyboard walkthrough:\n{}",
        problems.join("\n")
    );
    let total: usize = empty.values().map(Vec::len).sum();
    let full_total: usize = full.values().map(Vec::len).sum();
    assert!(total >= 200, "only {total} Tab stops");
    assert!(
        full_total > total,
        "the populated pass reached no more stops ({full_total}) than the empty one ({total})"
    );
    if let Some(evidence) = ed.evidence
        && std::env::var_os("FORGE_WRITE_EVIDENCE").is_some()
    {
        let mut record = String::from(
            "# Keyboard-only walkthrough of every panel (M2-31)\n\n\
             Generated by `tools/forge-editor-bin/tests/test_editor_panels_audit.rs` \
             (`keyboard_only_walkthrough_of_every_panel`, `FORGE_WRITE_EVIDENCE=1`). Each panel \
             is opened alone, focus goes to its first control, then Tab until focus returns; \
             every visible focusable widget was reached and every stop has a name. Two passes: \
             an empty project in an editor with no services, and a populated project in the \
             editor as the binary wires it (asset database, connect services, in-memory team \
             server and licence; `forge_editor_bin::wiring`) with a user config directory, so \
             the first-run tips show: two entities, the child carrying every built-in component \
             and selected, an image imported into a folder, a team created. In the populated \
             pass every panel has at least one stop.\n\n",
        );
        record_pass(&mut record, "Pass 1: empty project", &empty);
        record_pass(&mut record, "Pass 2: populated project", &full);
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(evidence);
        std::fs::write(&out, record).unwrap_or_else(|x| panic!("{}: {x}", out.display()));
    }
    println!(
        "keyboard walkthrough: {total} Tab stops on an empty project, {full_total} on a populated \
         one, across {} panels",
        e.panels.len()
    );
}

pub fn control_focus_trap(ed: &Edition) {
    let t = TestPanels::new(vec![("test.trap", trapping_panel)]);
    let mut e = editor(ed, &[&t]);
    let err = walk(&mut e, "test.trap").expect_err("a trapping focus scope passed");
    assert!(err.contains("never reached"), "{err}");
}

// ---- pseudo-locale: every visible string is a localisation key (M2-31) ---------------------

/// Data the empty project and this machine put on screen, with why it is not UI text: it is
/// shown as it is, never translated. Each entry must be seen (no stale entry).
const DATA: &[(&str, &str)] = &[
    (
        "Master",
        "the audio mixer's master bus: a bus is named by the project, like an entity",
    ),
    ("Forge", "the product's name, the application menu's title"),
    (
        "Entity",
        "the value of the project setting editor.new_entity_name",
    ),
];

/// Data one panel shows, with why: allowed in that panel only, so the same word hard-coded in
/// another panel is still found. Each word must be seen there (no stale entry).
const PANEL_DATA: &[PanelData] = &[(
    "forge.anim_graph",
    &[
        "Idle",
        "Walk",
        "Run",
        "StrafeLeft",
        "StrafeRight",
        "WalkBack",
        "Jump",
        "Fall",
        "Land",
        "Wave",
    ],
    "the motion (clip) names of the in-memory animation library (D-4): asset names, shown as \
     the library names them, like an entity's name",
)];

/// Unit symbols longer than two letters (`forge_units` gives them, like `m` and `W`): data,
/// shown as they are.
const UNIT_SYMBOLS: &[&str] = &["deg"];

/// The words of `s` a translator would translate that were not looked up: pseudo-localised
/// segments (`[...]`) removed, identifiers (`forge.tool.move`, `0x5eed`, `EDITOR-0006`),
/// numbers and short unit symbols (`m`, `dB`) skipped, and the data words of [`DATA`] allowed.
fn untranslated_words(s: &str, seen: &mut BTreeSet<&'static str>) -> Vec<String> {
    // Remove pseudo segments: '[' ... ' ' filler ']' as l10n::pseudo_localise makes them.
    let mut rest = String::new();
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => rest.push(c),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for token in rest.split(|c: char| {
        c.is_whitespace() || ",;()\u{2014}\u{b7}\u{2192}\u{201c}\u{201d}\"'".contains(c)
    }) {
        let token = token.trim_end_matches([':', '.', '!', '?']);
        let ident = token.contains(':')
            || token.contains('.') && token.chars().any(char::is_alphabetic)
            || token.contains('_')
            || token.contains("::")
            || token.chars().any(|c| c.is_ascii_digit()) && token.chars().any(char::is_alphabetic)
            || token.chars().filter(|c| *c == '-').count() > 0
                && token.chars().any(|c| c.is_ascii_digit());
        if ident {
            continue;
        }
        for word in token.split(|c: char| !c.is_alphabetic()) {
            if word.chars().count() < 3 || UNIT_SYMBOLS.contains(&word) {
                continue;
            }
            if let Some((d, _)) = DATA.iter().find(|(d, _)| *d == word) {
                seen.insert(d);
                continue;
            }
            out.push(word.to_string());
        }
    }
    out
}

/// Check one widget's visible strings, and those of the virtual children its AccessKit node
/// lists (the live rows of a virtualised tree, list or grid: hierarchy, undo history,
/// notifications, console, asset grid, …); how many were checked.
fn check_strings(
    e: &Editor,
    id: WidgetId,
    place: &str,
    extra: &[PanelData],
    seen_data: &mut BTreeSet<&'static str>,
    problems: &mut Vec<String>,
) -> usize {
    let mut checked = check_node_strings(e, id, place, extra, seen_data, problems);
    for v in e.rig.h.ui.a11y_virtual_children(id) {
        checked += check_node_strings(e, v, place, extra, seen_data, problems);
    }
    checked
}

/// Check the strings of one AccessKit node (a widget's or a virtual child's).
fn check_node_strings(
    e: &Editor,
    id: WidgetId,
    place: &str,
    extra: &[PanelData],
    seen_data: &mut BTreeSet<&'static str>,
    problems: &mut Vec<String>,
) -> usize {
    let Some(n) = e.rig.h.ui.a11y_node(id) else {
        return 0;
    };
    // Text a build or a file carries, shown exactly as written (Label::verbatim).
    if n.class_name() == Some(forge_ui::widgets::VERBATIM_CLASS) {
        return 0;
    }
    let mut checked = 0;
    let texts = [n.label(), n.value(), n.description(), n.placeholder()];
    for s in texts.into_iter().flatten() {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        checked += 1;
        if forge_ui::l10n::is_pseudo(s) {
            continue;
        }
        let mut words = untranslated_words(s, seen_data);
        words.retain(|w| {
            let data = PANEL_DATA
                .iter()
                .chain(extra)
                .filter(|(p, _, _)| *p == place)
                .flat_map(|(_, ws, _)| ws.iter())
                .find(|d| **d == w.as_str());
            if let Some(d) = data {
                seen_data.insert(d);
            }
            data.is_none()
        });
        if !words.is_empty() {
            let line = format!("{place}: \u{201c}{s}\u{201d} ({})", words.join(" "));
            if !problems.contains(&line) {
                problems.push(line);
            }
        }
    }
    checked
}

/// Make the editor refuse something the way a person meets it: rename the selected entity
/// to nothing in the hierarchy. The bus refuses it (`CMD-*`, `BadName`), the shell posts
/// the problem with its next step: a toast in the chrome and a row in Notifications.
fn provoke_a_refusal(e: &mut Editor) {
    let Some(pop) = &e.pop else {
        return;
    };
    let child = pop.child;
    if open(e, "forge.hierarchy").is_err() {
        return;
    }
    let Some(frame) = e.rig.panel_frame("forge.hierarchy") else {
        return;
    };
    let tree = subtree(&e.rig.h.ui, frame).into_iter().find(|i| {
        e.rig
            .h
            .ui
            .widget::<forge_ui::widgets::VirtualTree>(*i)
            .is_some()
    });
    if let Some(tree) = tree {
        e.rig.h.ui.raise(
            tree,
            forge_ui::widgets::RowRenamed {
                view: tree,
                key: child.0,
                name: String::new(),
            },
        );
        e.rig.turn();
        e.rig.settle();
    }
}

/// Every visible string of every panel under the pseudo-locale that was not looked up, read
/// from the AccessKit tree: each visible widget's node and the virtual rows it lists — on
/// an empty project, or (`full`) on the populated one with its tips, after a refusal (so its
/// toast and its notification row are read too).
fn hardcoded_strings(
    ed: &Edition,
    extra: &[&dyn SourcePlugin],
    full: bool,
) -> (Vec<String>, usize) {
    forge_ui::l10n::set_ui_locale(forge_ui::l10n::PSEUDO_LOCALE, BTreeMap::new());
    let mut e = if full {
        populated(ed, extra, "pseudo")
    } else {
        editor(ed, extra)
    };
    assert_covers(ed, &e);
    if full {
        let before = e.rig.shell.session().notifications.len();
        provoke_a_refusal(&mut e);
        let after = e.rig.shell.session().notifications.len();
        assert!(after > before, "the refusal posted no notification");
    }
    let mut seen_data = BTreeSet::new();
    let mut problems = Vec::new();
    let mut checked = 0;
    for p in e.panels.clone() {
        let frame = match open(&mut e, &p) {
            Ok(f) => f,
            Err(x) => {
                problems.push(x);
                continue;
            }
        };
        for id in visible(&e, frame) {
            checked += check_strings(&e, id, &p, ed.panel_data, &mut seen_data, &mut problems);
        }
    }
    // The editor's chrome around the panels: menu bar, toolbar, dock tabs, status bar.
    let root = e.rig.h.ui.root();
    for id in subtree(&e.rig.h.ui, root) {
        if e.rig.h.ui.is_visible(id) {
            checked += check_strings(&e, id, "chrome", &[], &mut seen_data, &mut problems);
        }
    }
    forge_ui::l10n::clear_ui_locale();
    if extra.is_empty() && !full {
        for (d, why) in DATA {
            if !seen_data.contains(d) {
                problems.push(format!("stale DATA entry {d:?} ({why}): nothing shows it"));
            }
        }
        for (p, words, why) in PANEL_DATA {
            for d in *words {
                if !seen_data.contains(d) {
                    problems.push(format!(
                        "stale PANEL_DATA entry {d:?} of {p} ({why}): the panel does not show it"
                    ));
                }
            }
        }
    }
    (problems, checked)
}

pub fn pseudo_locale(ed: &Edition) {
    let (problems, checked) = hardcoded_strings(ed, &[], false);
    assert!(
        problems.is_empty(),
        "{} visible strings were not looked up (hard-coded), empty project:\n{}",
        problems.len(),
        problems.join("\n")
    );
    assert!(checked >= 300, "only {checked} strings checked");
    let (problems, full_checked) = hardcoded_strings(ed, &[], true);
    assert!(
        problems.is_empty(),
        "{} visible strings were not looked up (hard-coded), populated project:\n{}",
        problems.len(),
        problems.join("\n")
    );
    assert!(
        full_checked > checked,
        "the populated pass read no more strings ({full_checked}) than the empty one ({checked})"
    );
    println!(
        "pseudo-locale: {checked} visible strings on an empty project, {full_checked} on a \
         populated one (tips, a refusal's toast and notification included), all looked up"
    );
}

fn hardcoded_panel(cx: &mut PanelCx) {
    cx.never_empty("a label");
    cx.add(|b, parent| {
        b.add(
            parent,
            "text",
            NodeStyle::leaf(),
            forge_ui::widgets::Label::new("Hard-coded text"),
        )?;
        Ok(())
    });
}

pub fn control_literal_string(ed: &Edition) {
    let t = TestPanels::new(vec![("test.hardcoded", hardcoded_panel)]);
    let (problems, _) = hardcoded_strings(ed, &[&t], false);
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("test.hardcoded:") && p.contains("Hard-coded text")),
        "{problems:?}"
    );
}

/// A panel whose virtualised list has one hard-coded row: the list's own name is looked up,
/// so only the row (a virtual AccessKit child, not a widget) carries the literal.
fn hardcoded_row_panel(cx: &mut PanelCx) {
    cx.never_empty("a list");
    cx.add(|b, parent| {
        let mut t = forge_ui::widgets::VirtualTree::list(forge_ui::tr!("Rows"));
        t.push(None, 1, forge_ui::widgets::RowItem::new("Hard-coded row"));
        b.add(parent, "rows", NodeStyle::leaf().grow(1.0), t)?;
        Ok(())
    });
}

pub fn control_literal_row(ed: &Edition) {
    let t = TestPanels::new(vec![("test.hardcoded_row", hardcoded_row_panel)]);
    let (problems, _) = hardcoded_strings(ed, &[&t], false);
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("test.hardcoded_row:") && p.contains("Hard-coded row")),
        "{problems:?}"
    );
}

// ---- idle with every panel open (§21.22 ui_idle_zero_redraw, gate C-ui-idle-every-panel) ---

/// Frames and wakeups over 10 s of injected idle with every panel open side by side
/// (all visible) or stacked as tabs (one visible, the rest hidden), a text field focused
/// and its caret timeout expired.
fn idle_every_panel(ed: &Edition, extra: &[&dyn SourcePlugin], visible_all: bool) -> (u64, u64) {
    let mut e = editor(ed, extra);
    assert_covers(ed, &e);
    let panels: Vec<&str> = e.panels.iter().map(String::as_str).collect();
    if visible_all {
        e.rig.show_panels(&panels).unwrap_or_else(|x| panic!("{x}"));
    } else {
        use forge_ui::dock::{DockNode, Layout};
        e.rig
            .shell
            .dock_mut()
            .set_layout(Layout::new(DockNode::tabs(&panels)));
        e.rig.settle();
    }
    // A focused text field (the hierarchy's filter when shown, else any), caret expired.
    let field = subtree(&e.rig.h.ui, e.rig.h.ui.root())
        .into_iter()
        .find(|i| {
            e.rig.h.ui.is_visible(*i)
                && e.rig
                    .h
                    .ui
                    .a11y_node(*i)
                    .is_some_and(|n| n.role() == Role::TextInput)
        });
    if let Some(f) = field {
        e.rig.h.ui.set_focus(Some(f), true);
    }
    e.rig.turn();
    e.rig.advance(Duration::from_secs(6));
    e.rig.advance(Duration::from_secs(3));
    e.rig.advance(Duration::from_secs(10))
}

pub fn idle(ed: &Edition) {
    for all in [true, false] {
        let (frames, wakeups) = idle_every_panel(ed, &[], all);
        assert_eq!(
            (frames, wakeups),
            (0, 0),
            "every panel open ({}): an idle editor draws nothing and never wakes",
            if all {
                "all visible"
            } else {
                "as tabs, one visible"
            }
        );
    }
    println!("idle with every panel open: 0 frames, 0 wakeups over 10 s (all visible; as tabs)");
}

fn spinning_panel(cx: &mut PanelCx) {
    cx.never_empty("a spinner");
    cx.add(|b, parent| {
        let live = b.signal(true);
        b.add(
            parent,
            "spin",
            NodeStyle::leaf(),
            forge_ui::widgets::Spinner::new(live, "Working"),
        )?;
        Ok(())
    });
}

pub fn control_spinning(ed: &Edition) {
    let t = TestPanels::new(vec![("test.spinning", spinning_panel)]);
    let (frames, wakeups) = idle_every_panel(ed, &[&t], true);
    assert!(
        frames > 0 && wakeups > 0,
        "an always-animating panel was not seen: {frames} frames, {wakeups} wakeups"
    );
}
