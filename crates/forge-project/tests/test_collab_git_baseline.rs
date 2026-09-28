//! WP-U10: the team baseline is a **real** `ProjectStore` — here the Git store (WP-16, ADR
//! 0035) on a folder. Publishing is `ProjectStore::commit` with the envelopes that made the
//! change (Ch.37 §37.3), so the baseline's history is Git history carrying the command log;
//! a sandbox that fell behind is refused (a fast-forward), and a three-way merge over the
//! revisions it reads back finds the teammate's change and mine.

use std::collections::BTreeMap;
use std::sync::Arc;

use forge_cmd::{CommandEnvelope, CommandId, EditorCommand, EntityKey, Issuer, TxnId, Value};
use forge_project::ProjectError;
use forge_project::collab::{CollabBackend, CollabView, MemoryCollab, MemoryIdentity, Publish};
use forge_project::format::{EntityDoc, ProjectDoc};
use forge_project::merge::merge3;
use forge_store::RevId;

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("forge-collab-git-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn doc(pos: f64, colour: f64) -> ProjectDoc {
    ProjectDoc {
        settings: [("project.name".to_string(), Value::Text("Team".into()))]
            .into_iter()
            .collect(),
        entities: vec![EntityDoc {
            id: 0,
            name: "Lamp".into(),
            parent: None,
            properties: [
                ("pos".to_string(), Value::Float(pos)),
                ("colour".to_string(), Value::Float(colour)),
            ]
            .into_iter()
            .collect(),
        }],
    }
}

fn env(user: &str, n: u64, v: f64) -> CommandEnvelope {
    CommandEnvelope {
        id: CommandId(n),
        txn: TxnId(n),
        issuer: Issuer::Human { user: user.into() },
        cmd: EditorCommand::SetProperty {
            entity: EntityKey(0),
            path: "pos".into(),
            value: Value::Float(v),
        },
    }
}

fn publish(author: &str, base: Option<RevId>, d: ProjectDoc, e: Vec<CommandEnvelope>) -> Publish {
    Publish {
        author: author.into(),
        base,
        message: format!("{author}'s work"),
        doc: d,
        envelopes: e,
        paths: Vec::new(),
        summary: Vec::new(),
        at_ms: 1,
    }
}

#[test]
fn publishing_commits_to_a_git_baseline_and_a_stale_sandbox_merges_three_way() {
    let dir = tmp("baseline");
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    let open = forge_project::host::first_party_opener().unwrap_or_else(|e| panic!("{e}"));
    let store =
        open(&format!("git:{}", dir.display()), "forge-server").unwrap_or_else(|e| panic!("{e}"));
    let server = MemoryCollab::new(store, Arc::new(MemoryIdentity::new()));
    assert!(server.backend().contains("git"), "{}", server.backend());
    assert!(
        server.backend().contains("in-memory stand-in"),
        "labelled D-4"
    );
    let r1 = server
        .publish(publish(
            "ada",
            None,
            doc(0.0, 1.0),
            vec![env("ada", 1, 0.0)],
        ))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        dir.join(".forge").join("git").exists(),
        "a real Git repository"
    );
    // Bob, on r1, recolours the lamp and publishes.
    let r2 = server
        .publish(publish(
            "bob",
            Some(r1),
            doc(0.0, 2.0),
            vec![env("bob", 2, 0.0)],
        ))
        .unwrap_or_else(|e| panic!("{e}"));
    // Ada, still on r1, moved it: her publish is refused, nothing committed.
    let refused = server.publish(publish(
        "ada",
        Some(r1),
        doc(5.0, 1.0),
        vec![env("ada", 3, 5.0)],
    ));
    assert!(
        matches!(refused, Err(ProjectError::BaselineMoved(_))),
        "{refused:?}"
    );
    assert_eq!(server.head(), Some(r2));
    // The history reads back from Git with who did what.
    let revs = server.revisions(None, 10);
    assert_eq!(revs.len(), 2);
    assert_eq!(revs[1].issuers, vec!["human:bob".to_string()]);
    // The rebase: base r1, theirs r2 (read back from the Git store), mine = her sandbox.
    let base = server.doc_at(Some(r1)).unwrap_or_else(|e| panic!("{e}"));
    let theirs = server.doc_at(Some(r2)).unwrap_or_else(|e| panic!("{e}"));
    let m = merge3(&base, &doc(5.0, 1.0), &theirs, &BTreeMap::new());
    assert!(m.is_clean(), "{:?}", m.conflicts);
    let lamp = &m.doc.entities[0];
    assert_eq!(lamp.properties.get("pos"), Some(&Value::Float(5.0)));
    assert_eq!(lamp.properties.get("colour"), Some(&Value::Float(2.0)));
    // Rebased, her publish is a fast-forward again.
    let r3 = server
        .publish(publish(
            "ada",
            Some(r2),
            m.doc.clone(),
            vec![env("ada", 3, 5.0)],
        ))
        .unwrap_or_else(|e| panic!("{e}"));
    // A fresh view of the same Git folder sees the same head: it is on disk, not in memory.
    let again =
        open(&format!("git:{}", dir.display()), "someone-else").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(again.head().ok().flatten(), Some(r3));
    let _ = std::fs::remove_dir_all(&dir);
}
