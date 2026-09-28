//! Shared set-up: the editor with the core, scene and collaboration panels (stand-ins for the
//! rest) in a headless `Rig`, its core attached to a labelled in-memory team server and
//! identity database (D-4) over an in-memory baseline store; teammates are other editors
//! (cores) on the same server.
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use forge_cmd::{EditorCommand, EntityKey, Issuer};
use forge_editor::client::BusClient;
use forge_editor::collab::licence::MemoryEntitlement;
use forge_editor::core::{CollabAttach, EditorCore, LocalBus, SharedCore};
use forge_editor::presets::builtin_preset;
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_collab::PanelsCollab;
use forge_panels_core::PanelsCore;
use forge_panels_scene::PanelsScene;
use forge_plugin::SourcePlugin;
use forge_project::collab::{CollabView, InviteTo, MemoryCollab, MemoryIdentity, Role};
use forge_ui::widgets::{Label, SelectionChanged, VirtualTree};
use forge_ui::{Key, WidgetId};

pub const DAY: u64 = 86_400_000;
pub const NOW: u64 = 20_000 * DAY;

/// The team server the editors share.
pub struct Server {
    pub identity: Arc<MemoryIdentity>,
    pub server: Arc<MemoryCollab>,
    pub licence: Arc<MemoryEntitlement>,
}

pub fn server() -> Server {
    static N: AtomicU64 = AtomicU64::new(0);
    let identity = Arc::new(MemoryIdentity::new());
    let name = format!(
        "collab-panels-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let store = forge_project::memory::open_named(&name, "forge-server");
    let server = Arc::new(MemoryCollab::new(store, identity.clone()));
    let licence = Arc::new(MemoryEntitlement::team_for_a_year(NOW));
    licence.set_now(Some(NOW));
    Server {
        identity,
        server,
        licence,
    }
}

pub fn attach(s: &Server, core: &SharedCore, user: &str) {
    EditorCore::attach_collab(
        core,
        CollabAttach {
            identity: s.identity.clone(),
            server: s.server.clone(),
            licence: s.licence.clone(),
            owner: user.into(),
            email: Some(format!("{user}@studio.example")),
        },
    );
}

pub fn config() -> ShellConfig {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let scene = PanelsScene::new().unwrap_or_else(|e| panic!("{e}"));
    let collab = PanelsCollab::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_scene::panel_ids());
    real.extend(forge_panels_collab::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let all: Vec<&dyn SourcePlugin> = vec![&core, &scene, &collab, &stand_in];
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    assemble(preset, &all, &[], None).unwrap_or_else(|e| panic!("{e}"))
}

/// A rig (the person is `tester`, the shell's issuer) on `s`; `edit` adjusts the services.
pub fn rig_with(s: &Server, panels: &[&str], edit: impl FnOnce(&mut ShellConfig)) -> Rig {
    let core = EditorCore::new();
    attach(s, &core, "tester");
    let mut cfg = config();
    cfg.services.collab.attach(&core);
    edit(&mut cfg);
    let mut rig = Rig::with_core(cfg, core).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(panels).unwrap_or_else(|e| panic!("{e}"));
    rig
}

pub fn rig(s: &Server, panels: &[&str]) -> Rig {
    rig_with(s, panels, |_| {})
}

/// A teammate: another editor (core) on the same server, and their client.
pub struct Mate {
    pub core: SharedCore,
    pub ui: LocalBus,
}

pub fn mate(s: &Server, user: &str) -> Mate {
    let core = EditorCore::new();
    attach(s, &core, user);
    let ui = EditorCore::connect(&core, Issuer::Human { user: user.into() });
    Mate { core, ui }
}

/// Send a command as the teammate; the refusal or failed outcome, if any.
pub fn mate_send(m: &mut Mate, cmd: EditorCommand) -> Result<(), String> {
    let before = EditorCore::collab_status(&m.core).map_or(0, |s| s.outcomes.len());
    m.ui.apply(cmd, None);
    let p = m.ui.pump();
    if let Some(r) = p.refused.first() {
        return Err(r.rejection.error.to_string());
    }
    let st = EditorCore::collab_status(&m.core);
    match st.as_ref().and_then(|x| x.outcomes.last()) {
        Some(o) if st.as_ref().is_some_and(|x| x.outcomes.len() != before) && !o.ok => {
            Err(o.message.clone())
        }
        _ => Ok(()),
    }
}

/// A teammate joins the rig's team with a fresh code as `role` (minted by the server's Owner
/// through the identity database directly: the rig's panels are the thing under test).
pub fn join(s: &Server, user: &str, role: Role) -> Mate {
    use forge_project::collab::{IdentityBackend, InviteSpec};
    let team = s.server.team().unwrap_or_else(|| panic!("no team yet"));
    let inv = s
        .identity
        .invite(
            &team,
            InviteSpec {
                email: None,
                role,
                paths: Vec::new(),
            },
            "tester",
            NOW,
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let InviteTo::Code(code) = inv.to else {
        panic!("a code invite")
    };
    let mut m = mate(s, user);
    mate_send(&mut m, forge_editor::collab::accept_command(&code))
        .unwrap_or_else(|e| panic!("{user} joins: {e}"));
    m
}

pub fn spawn(m: &mut Mate, name: &str, parent: Option<EntityKey>) -> EntityKey {
    let k = EditorCore::read(&m.core, |p| p.next_key());
    mate_send(
        m,
        EditorCommand::Spawn {
            name: name.into(),
            parent,
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    k
}

/// The widget at `path` (keys) under a panel's frame.
pub fn part(rig: &Rig, panel: &str, path: &[&str]) -> WidgetId {
    let mut id = rig
        .panel_frame(panel)
        .unwrap_or_else(|| panic!("{panel} is not open"));
    for k in path {
        id = id.child(&Key::Str((*k).into()));
    }
    assert!(rig.h.ui.contains(id), "{panel}: no widget at {path:?}");
    id
}

/// Under the panel's `content` column.
pub fn at(rig: &Rig, panel: &str, path: &[&str]) -> WidgetId {
    let mut p = vec!["content"];
    p.extend_from_slice(path);
    part(rig, panel, &p)
}

/// Press a button (as its activation would) and let the editor settle.
pub fn press(rig: &mut Rig, id: WidgetId) {
    rig.h.ui.raise(id, forge_ui::widgets::Pressed(id));
    rig.turn();
    rig.settle();
}

/// A real pointer click on a widget, and settle.
pub fn click(rig: &mut Rig, id: WidgetId) {
    rig.h.click(id);
    rig.turn();
    rig.settle();
}

/// Type into a text field.
pub fn type_into(rig: &mut Rig, id: WidgetId, text: &str) {
    rig.h.focus(id);
    rig.h.type_text(text);
    rig.turn();
    rig.settle();
}

/// Select the first row whose label contains `needle`; its key.
pub fn select_containing(rig: &mut Rig, list: WidgetId, needle: &str) -> u64 {
    let key = VirtualTree::edit(&mut rig.h.ui, list, |t| {
        (0..t.row_count())
            .filter_map(|r| t.key_at(r))
            .find(|k| t.label_of(*k).is_some_and(|l| l.contains(needle)))
    })
    .flatten()
    .unwrap_or_else(|| panic!("no row contains {needle:?}: {:?}", rows(rig, list)));
    VirtualTree::edit(&mut rig.h.ui, list, |t| t.select(&[key]));
    rig.h.ui.raise(
        list,
        SelectionChanged {
            view: list,
            keys: vec![key],
        },
    );
    rig.turn();
    rig.settle();
    key
}

/// Every row label of a list, in order.
pub fn rows(rig: &mut Rig, list: WidgetId) -> Vec<String> {
    VirtualTree::edit(&mut rig.h.ui, list, |t| {
        (0..t.row_count())
            .filter_map(|r| t.key_at(r).and_then(|k| t.label_of(k)).map(str::to_string))
            .collect()
    })
    .unwrap_or_default()
}

/// A label's text.
/// A label's text, or an empty state's message (a panel's notice is its empty state).
pub fn label(rig: &Rig, id: WidgetId) -> String {
    rig.h
        .ui
        .widget::<Label>(id)
        .map(|l| l.text(rig.h.ui.rt()))
        .or_else(|| {
            rig.h
                .ui
                .widget::<forge_ui::widgets::EmptyState>(id)
                .map(|l| l.message(rig.h.ui.rt()))
        })
        .unwrap_or_default()
}

/// Is a widget (or one of its ancestors) hidden?
pub fn hidden(rig: &Rig, id: WidgetId) -> bool {
    let mut cur = Some(id);
    while let Some(w) = cur {
        if rig.h.ui.is_hidden(w) {
            return true;
        }
        cur = rig.h.ui.parent(w);
    }
    false
}

/// Let live feeds deliver (they refresh at most 10 times a second).
pub fn feed(rig: &mut Rig) {
    rig.advance(Duration::from_millis(400));
    rig.settle();
}

/// The rig's refused commands so far, as notifications ("… CMD-0014 …").
pub fn notices(rig: &Rig) -> Vec<String> {
    rig.shell
        .session()
        .notifications
        .history()
        .map(|n| {
            format!(
                "{} {} {}",
                n.code.clone().unwrap_or_default(),
                n.title,
                n.detail
            )
        })
        .collect()
}

/// Press the button at `path` under a panel's content.
pub fn tap(rig: &mut Rig, panel: &str, path: &[&str]) {
    let id = at(rig, panel, path);
    press(rig, id);
}

/// Type into the text field at `path` under a panel's content.
pub fn write(rig: &mut Rig, panel: &str, path: &[&str], text: &str) {
    let id = at(rig, panel, path);
    type_into(rig, id, text);
}
