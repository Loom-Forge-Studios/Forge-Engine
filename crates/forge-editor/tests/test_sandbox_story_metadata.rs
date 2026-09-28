//! `test_sandbox_story_metadata` (Ch.37 I19, Ch.33 §33.4; WP-U14, ADR 0041): **a publish's
//! folded command list is metadata; the baseline gets the sandbox's document.**
//!
//! A sandbox keeps its deltas folded (a drag is its last frame), leaves undone and
//! cancelled transactions out, never holds the pulls that brought teammates' work in, and
//! names entities by the keys they had when the command ran — keys a later pull may change.
//! That list rides with a publish as its story (summary, command count, audit). Replaying it
//! as commands would not rebuild what the person published, so nothing does: the server
//! commits `Publish::doc`.
//!
//! The check is the case where a replay does real damage. Ada and Bob each add an entity
//! with the same fresh key; Bob publishes first; Ada's pull re-keys her Tree (a teammate
//! used the key) and she publishes, her story still naming the Tree by the old key. The
//! baseline must hold Bob's Rock untouched and Ada's Tree with her mass, equal to Ada's
//! sandbox — and the story is still carried (the revision counts its commands). The test
//! also shows the story is not a replayable log: applied to the base on a scratch bus, it
//! puts Ada's mass on Bob's Rock.
//!
//! Positive control (W2): `positive_control_publishing_a_replayed_story_fails` — a publish
//! that replays the story onto the base (`CollabFaults::publish_by_replay`) edits a
//! teammate's entity, and the check fails.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use forge_cmd::{CommandSink, EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::collab::licence::MemoryEntitlement;
use forge_editor::collab::{self as c, CollabFaults};
use forge_editor::core::{CollabAttach, EditorCore, LocalBus, SharedCore};
use forge_project::collab::{
    CollabBackend, CollabView, IdentityBackend, InviteTo, MemoryCollab, MemoryIdentity, Role,
};
use forge_project::format::ProjectDoc;

const NOW: u64 = 20_000 * 86_400_000;

struct Server {
    name: String,
    identity: Arc<MemoryIdentity>,
    server: Arc<MemoryCollab>,
    licence: Arc<MemoryEntitlement>,
}

struct Editor {
    core: SharedCore,
    ui: LocalBus,
}

fn server() -> Server {
    static N: AtomicU64 = AtomicU64::new(0);
    let identity = Arc::new(MemoryIdentity::new());
    let name = format!(
        "sandbox-story-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let store = forge_project::memory::open_named(&name, "forge-server");
    let server = Arc::new(MemoryCollab::new(store, identity.clone()));
    let licence = Arc::new(MemoryEntitlement::team_for_a_year(NOW));
    licence.set_now(Some(NOW));
    Server {
        name,
        identity,
        server,
        licence,
    }
}

fn editor(s: &Server, user: &str, faults: CollabFaults) -> Editor {
    let core = EditorCore::new();
    EditorCore::attach_collab(
        &core,
        CollabAttach {
            identity: s.identity.clone(),
            server: s.server.clone(),
            licence: s.licence.clone(),
            owner: user.into(),
            email: Some(format!("{user}@studio.example")),
        },
    );
    EditorCore::set_collab_faults(&core, faults);
    let ui = EditorCore::connect(&core, Issuer::Human { user: user.into() });
    Editor { core, ui }
}

fn send(e: &mut Editor, cmd: EditorCommand) -> Result<(), String> {
    let what = cmd.label();
    let before = EditorCore::collab_status(&e.core).map_or(0, |s| s.outcomes.len());
    e.ui.apply(cmd, None);
    if let Some(r) = e.ui.pump().refused.first() {
        return Err(format!("{what} refused: {}", r.rejection.error));
    }
    let st = EditorCore::collab_status(&e.core);
    match st.as_ref().and_then(|s| s.outcomes.last()) {
        Some(o) if st.as_ref().is_some_and(|s| s.outcomes.len() != before) && !o.ok => {
            Err(format!("{what}: {}", o.message))
        }
        _ => Ok(()),
    }
}

fn spawn(e: &mut Editor, name: &str) -> Result<EntityKey, String> {
    let k = EditorCore::read(&e.core, |p| p.next_key());
    send(
        e,
        EditorCommand::Spawn {
            name: name.into(),
            parent: None,
        },
    )?;
    Ok(k)
}

fn sorted(mut d: ProjectDoc) -> ProjectDoc {
    d.entities.sort_by_key(|e| e.id);
    d
}

fn mass_of(d: &ProjectDoc, name: &str) -> Option<Value> {
    d.entities
        .iter()
        .find(|e| e.name == name)
        .and_then(|e| e.properties.get("mass").cloned())
}

/// `story` applied to `base` on a scratch bus (what a replay would publish).
fn replayed(base: &ProjectDoc, story: &[forge_cmd::CommandEnvelope]) -> Result<ProjectDoc, String> {
    let mut bus = EditorCore::editor_bus();
    let load = base.load_command().map_err(|e| e.to_string())?;
    let e = bus.envelope(Issuer::Test, load);
    bus.apply(e).map_err(|r| r.error.to_string())?;
    for s in story {
        let e = bus.envelope(s.issuer.clone(), s.cmd.clone());
        let _ = bus.apply(e);
    }
    Ok(ProjectDoc::from_project(bus.project()))
}

fn check(faults: CollabFaults) -> Result<(), String> {
    let s = server();
    let mut ada = editor(&s, "ada", faults);
    spawn(&mut ada, "Lamp")?;
    send(&mut ada, c::create_command("Studio"))?;
    // Bob joins with a join code.
    send(&mut ada, c::invite_command(None, Role::Developer, &[]))?;
    let team = s.server.team().ok_or("no team")?;
    let code = s
        .identity
        .team_record(&team)
        .and_then(|t| {
            t.invites.iter().rev().find_map(|i| match &i.to {
                InviteTo::Code(c) => Some(c.clone()),
                InviteTo::Email(_) => None,
            })
        })
        .ok_or("no join code")?;
    let mut bob = editor(&s, "bob", CollabFaults::default());
    send(&mut bob, c::accept_command(&code))?;

    // The same fresh key on both sides.
    let tree = spawn(&mut ada, "Tree")?;
    let rock = spawn(&mut bob, "Rock")?;
    if tree != rock {
        return Err(format!(
            "set-up: the fresh keys differ ({tree} / {rock}); the case needs them equal"
        ));
    }
    send(
        &mut ada,
        EditorCommand::SetProperty {
            entity: tree,
            path: "mass".into(),
            value: Value::Float(7.0),
        },
    )?;
    send(&mut bob, c::publish_command("Rock"))?;
    // Ada pulls: her Tree is re-keyed; her story still names it by the old key.
    send(&mut ada, c::pull_command(None))?;
    let tree_now = EditorCore::read(&ada.core, |p| {
        p.entities()
            .find(|(_, e)| e.name() == "Tree")
            .map(|(k, _)| k)
    })
    .ok_or("ada's Tree is gone")?;
    if tree_now == tree {
        return Err("set-up: the pull did not re-key ada's Tree".into());
    }
    let base_rev = s.server.head();
    let base_doc = (*s.server.doc_at(base_rev).map_err(|e| e.to_string())?).clone();
    send(&mut ada, c::publish_command("Tree"))?;

    let head = sorted(
        (*s.server
            .doc_at(s.server.head())
            .map_err(|e| e.to_string())?)
        .clone(),
    );
    if mass_of(&head, "Rock").is_some() {
        return Err(
            "the baseline put ada's mass on bob's Rock: a teammate's entity was edited by a replayed story"
                .into(),
        );
    }
    if mass_of(&head, "Tree") != Some(Value::Float(7.0)) {
        return Err(format!(
            "the baseline's Tree has mass {:?}",
            mass_of(&head, "Tree")
        ));
    }
    let mine = sorted(EditorCore::read(&ada.core, ProjectDoc::from_project));
    if head != mine {
        return Err("the baseline is not ada's sandbox document".into());
    }
    // The story is carried (metadata)...
    let revs = s.server.revisions(base_rev, usize::MAX);
    let story_len = revs.last().map_or(0, |r| r.commands);
    if story_len == 0 {
        return Err("the revision carries no story".into());
    }
    // ...and it is not a replayable log: applied to the base, it edits the Rock.
    let store = forge_project::memory::open_named(&s.name, "reader");
    let head_rev = s.server.head().ok_or("no head")?;
    let story = store.commands(&head_rev).map_err(|e| e.to_string())?;
    if story.len() != story_len {
        return Err(format!(
            "the story holds {} commands, the revision counts {story_len}",
            story.len()
        ));
    }
    let replay = replayed(&base_doc, &story)?;
    if mass_of(&replay, "Rock").is_none() {
        return Err(
            "set-up: replaying the story did not reach the Rock; the case shows nothing".into(),
        );
    }
    Ok(())
}

#[test]
fn the_baseline_gets_the_document_and_the_story_is_metadata() {
    check(CollabFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_publishing_a_replayed_story_fails() {
    let e = check(CollabFaults {
        publish_by_replay: true,
        ..CollabFaults::default()
    })
    .expect_err("a publish that replays the story must fail");
    assert!(e.contains("teammate's entity"), "{e}");
}
