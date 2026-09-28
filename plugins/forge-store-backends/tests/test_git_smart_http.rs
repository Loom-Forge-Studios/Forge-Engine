//! The Git store over Git's smart HTTP protocol (Ch.33.1 "any remote", M2-15), against real
//! `git`: the server is a small smart-HTTP front (what `git http-backend` does) over
//! `git upload-pack` / `git receive-pack`, so the packs this client sends are unpacked by
//! real Git and the packs it receives — deltified by `git repack` — are real Git's.

mod common;

use std::sync::Arc;

use common::{ROCK, Seen, git, p, push, script, serve_git, tmp};
use forge_store::parity::snapshot;
use forge_store::{Bytes, MemoryStore, ProjectStore, StoreError};
use forge_store_backends::git::{Credential, GitOptions, GitStore};

fn opts(dir: &std::path::Path, creds: &forge_store_backends::git::Credentials) -> GitOptions {
    GitOptions {
        cache_dir: dir.join("cache"),
        credentials: creds.clone(),
        ..GitOptions::new()
    }
}

#[test]
fn push_clone_and_pull_over_smart_http_with_sign_in() {
    let d = tmp("smart");
    let bare = d.join("server").join("orbits.git");
    std::fs::create_dir_all(&bare).unwrap_or_else(|e| panic!("{e}"));
    git(&bare, &["init", "--quiet", "--bare"]);
    let seen = Arc::new(Seen::default());
    let port = serve_git(bare.clone(), Some("gho_test".into()), Arc::clone(&seen));
    let url = format!("http://127.0.0.1:{port}/ada/orbits.git");
    let creds = forge_store_backends::git::Credentials::default();

    // Not signed in: refused, named, and nothing leaks into the message.
    let e = GitStore::open_remote(&url, "ada", &opts(&d.join("a"), &creds)).err();
    assert!(matches!(e, Some(StoreError::Unauthorized { .. })), "{e:?}");
    creds.set(&format!("127.0.0.1:{port}"), Credential::token("gho_test"));

    // Push ada's project (text, a binary, two revisions with command logs).
    let mut ada = GitStore::open_folder(d.join("ada"), "ada").unwrap_or_else(|e| panic!("{e}"));
    let ids = script(&mut ada).unwrap_or_else(|e| panic!("{e}"));
    let mut remote = GitStore::open_remote(&url, "ada", &opts(&d.join("a"), &creds))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        push(&ada, &mut remote).unwrap_or_else(|e| panic!("{e}")),
        ids
    );
    // Real Git unpacked it: a valid repository holding exactly ada's history.
    git(&bare, &["fsck", "--strict", "--no-dangling"]);
    assert_eq!(
        git(&bare, &["rev-parse", "refs/heads/main", "refs/forge/blobs"]),
        git(
            ada.git_dir(),
            &["rev-parse", "refs/heads/main", "refs/forge/blobs"]
        )
    );
    let reqs = seen.requests.lock().map(|r| r.clone()).unwrap_or_default();
    assert!(
        reqs.iter()
            .any(|r| r == "POST /ada/orbits.git/git-receive-pack"),
        "{reqs:?}"
    );

    // Make real Git's packs deltified, then clone through them.
    let mut bob = MemoryStore::new("bob");
    let view = GitStore::open_remote(&url, "bob", &opts(&d.join("b"), &creds))
        .unwrap_or_else(|e| panic!("{e}"));
    push(&view, &mut bob).unwrap_or_else(|e| panic!("{e}"));
    let big: String = (0..4000)
        .map(|i| format!("line {i} of the level\n"))
        .collect();
    for n in 0..3 {
        bob.write(
            &p("levels/big.ron"),
            Bytes::from(format!("{big}edit {n}\n").into_bytes()),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        bob.commit(&format!("edit {n}"), &[])
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let mut view = GitStore::open_remote(&url, "bob", &opts(&d.join("b"), &creds))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        push(&bob, &mut view)
            .unwrap_or_else(|e| panic!("{e}"))
            .len(),
        3
    );
    git(
        &bare,
        &[
            "repack",
            "-a",
            "-d",
            "-f",
            "--depth=50",
            "--window=50",
            "--quiet",
        ],
    );
    let packs = git(&bare, &["count-objects", "-v"]);
    assert!(packs.contains("packs: 1"), "{packs}");
    let verify = std::fs::read_dir(bare.join("objects").join("pack"))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x == "idx"))
        })
        .ok()
        .flatten()
        .map(|idx| git(&bare, &["verify-pack", "-v", &idx.display().to_string()]))
        .unwrap_or_default();
    assert!(
        verify.lines().any(|l| l.contains("chain length")),
        "real Git's pack holds deltas: {verify}"
    );
    let fresh = GitStore::open_remote(&url, "cy", &opts(&d.join("c"), &creds))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut cy = MemoryStore::new("cy");
    push(&fresh, &mut cy).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        snapshot(&cy).map(|s| s.revs).ok(),
        snapshot(&bob).map(|s| s.revs).ok(),
        "a clone through a deltified pack has every revision"
    );
    assert_eq!(cy.read(&p("assets/rock.png")).ok().as_deref(), Some(ROCK));
    assert!(
        cy.read(&p("levels/big.ron"))
            .is_ok_and(|b| b.ends_with(b"edit 2\n"))
    );

    // Pull is incremental: the next view fetches only what is new.
    let before = seen.requests.lock().map(|r| r.len()).unwrap_or(0);
    let again = GitStore::open_remote(&url, "cy", &opts(&d.join("c"), &creds))
        .unwrap_or_else(|e| panic!("{e}"));
    let after = seen
        .requests
        .lock()
        .map(|r| r[before..].to_vec())
        .unwrap_or_default();
    assert_eq!(
        after,
        ["GET /ada/orbits.git/info/refs?service=git-upload-pack"],
        "up to date: one ref read, no pack"
    );
    drop(again);

    // Two writers on one head: the second push is refused and changes nothing.
    let mut v1 = GitStore::open_remote(&url, "ada", &opts(&d.join("v1"), &creds))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut v2 = GitStore::open_remote(&url, "bob", &opts(&d.join("v2"), &creds))
        .unwrap_or_else(|e| panic!("{e}"));
    v1.write(&p("a.ron"), Bytes::from_static(b"A"))
        .unwrap_or_else(|e| panic!("{e}"));
    v1.commit("ada's", &[]).unwrap_or_else(|e| panic!("{e}"));
    v2.write(&p("b.bin"), Bytes::from_static(b"\0B"))
        .unwrap_or_else(|e| panic!("{e}"));
    v2.commit("bob's", &[]).unwrap_or_else(|e| panic!("{e}"));
    v1.publish().unwrap_or_else(|e| panic!("{e}"));
    let head = git(&bare, &["rev-parse", "refs/heads/main", "refs/forge/blobs"]);
    let e = v2.publish().err();
    assert!(matches!(e, Some(StoreError::RemoteMoved { .. })), "{e:?}");
    assert_eq!(
        git(&bare, &["rev-parse", "refs/heads/main", "refs/forge/blobs"]),
        head,
        "neither ref moved"
    );
    git(&bare, &["fsck", "--strict", "--no-dangling"]);
}

#[test]
fn a_server_that_is_not_smart_http_is_named() {
    let port = common::serve(|_| common::Resp::new(200, "text/html", "<html>a web page</html>"));
    let d = tmp("dumb");
    let e = GitStore::open_remote(
        &format!("http://127.0.0.1:{port}/x.git"),
        "ada",
        &opts(&d, &Default::default()),
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_default();
    assert!(
        e.starts_with("STORE-0010") && e.contains("not a smart-HTTP Git server"),
        "{e}"
    );
    let port = common::serve(|_| common::Resp::new(404, "text/plain", "no"));
    let e = GitStore::open_remote(
        &format!("http://127.0.0.1:{port}/x.git"),
        "ada",
        &opts(&d, &Default::default()),
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_default();
    assert!(e.contains("not found"), "{e}");
}
