//! The Git store (Ch.33.1, Ch.33.2, M2-15, ADR 0035): a project folder whose history is a
//! Git repository, and a Git remote on a path — checked against `MemoryStore` for the same
//! revision ids and against the `git` command line for a valid, well-laid-out repository.

mod common;

use common::{ROCK, git, p, push, script, tmp};
use forge_cmd::FixedClock;
use forge_store::parity::{differences, snapshot};
use forge_store::{Blake3, Bytes, MemoryStore, ProjectStore, RevRange, StoreError};
use forge_store_backends::git::{GitOptions, GitStore};

fn opts(dir: &std::path::Path) -> GitOptions {
    GitOptions {
        cache_dir: dir.join("cache"),
        ..GitOptions::new()
    }
}

#[test]
fn a_git_project_folder_holds_what_memory_holds_bit_for_bit() {
    let d = tmp("folder-parity");
    let mut g =
        GitStore::open_folder_with_clock(d.join("proj"), "ada", Box::new(FixedClock(5_000)))
            .unwrap_or_else(|e| panic!("{e}"));
    let mut m = MemoryStore::new("ada");
    let ids_g = script(&mut g).unwrap_or_else(|e| panic!("{e}"));
    let ids_m = script(&mut m).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(ids_g, ids_m, "revision ids are content, not backend");
    let a = snapshot(&g).unwrap_or_else(|e| panic!("{e}"));
    let b = snapshot(&m).unwrap_or_else(|e| panic!("{e}"));
    let diff = differences(&a, &b);
    assert!(diff.is_empty(), "{}", diff.join("\n"));
    assert_eq!(a.revs.len(), 2);
    assert_eq!(a.locks.len(), 1);
    // Reopened, it reads the same history (from Git alone, a fresh cache).
    drop(g);
    let g2 = GitStore::open_folder(d.join("proj"), "ada").unwrap_or_else(|e| panic!("{e}"));
    let c = snapshot(&g2).unwrap_or_else(|e| panic!("{e}"));
    assert!(differences(&a, &c).is_empty());
    assert_eq!(
        g2.history(RevRange::all()).map(|h| h[0].at_ms).ok(),
        Some(5_000)
    );
}

#[test]
fn the_history_is_a_valid_git_repository_laid_out_for_review() {
    let d = tmp("layout");
    let mut g = GitStore::open_folder(d.join("proj"), "ada").unwrap_or_else(|e| panic!("{e}"));
    let ids = script(&mut g).unwrap_or_else(|e| panic!("{e}"));
    let gd = g.git_dir().to_path_buf();
    // `git fsck --strict`: every object well-formed, every tree entry name acceptable to a
    // host that checks pushes (GitHub, GitLab and Gitea do).
    git(&gd, &["fsck", "--strict", "--no-dangling"]);
    let log = git(&gd, &["log", "--format=%an|%s", "refs/heads/main"]);
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        [
            "automation:sess-4|automation:sess-4 tuned the light",
            "human:ada|human:ada built the world"
        ],
        "one commit per revision, authored by its first issuer"
    );
    let body = git(&gd, &["log", "-1", "--format=%B", "refs/heads/main"]);
    assert!(
        body.contains(&format!("Forge-Revision: {}", ids[1])),
        "{body}"
    );
    let files = git(&gd, &["ls-tree", "-r", "--name-only", "refs/heads/main"]);
    let files: Vec<&str> = files.lines().collect();
    assert_eq!(
        files,
        [
            ".forge/commands.jsonl",
            ".forge/revision.ron",
            ".forge/tree",
            ".gitignore",
            "levels/one.ron",
            "scenes/main.ron"
        ],
        "text files and the metadata are in the tree; the binary is not"
    );
    // The text file is itself, diffable.
    assert_eq!(
        git(&gd, &["show", "refs/heads/main:scenes/main.ron"]),
        "Scene(\n  lights: 2,\n)\n"
    );
    // Generated ignore rules name the binary.
    let ignore = git(&gd, &["show", "refs/heads/main:.gitignore"]);
    assert!(
        ignore.contains("/.forge/\n") && ignore.contains("/assets/rock.png\n"),
        "{ignore}"
    );
    // The binary is an engine-native blob on the blobs chain, named by its address.
    let chain = git(&gd, &["ls-tree", "--name-only", "refs/forge/blobs"]);
    assert_eq!(chain.trim(), Blake3::of(ROCK).to_hex());
    assert_eq!(
        git(&gd, &["rev-list", "--count", "refs/forge/blobs"]).trim(),
        "1",
        "the second revision brought no new binary: no second blobs commit"
    );
    // The CRLF text of the first revision is stored byte for byte.
    let first = git(&gd, &["show", "refs/heads/main~1:project.ron"]);
    assert_eq!(first, "Project(preset: ThreeD)\r\n");
    // A plain clone of the branch never downloads the binary.
    let plain = d.join("plain");
    git(
        &d,
        &[
            "clone",
            "--quiet",
            "--bare",
            "--no-local",
            &gd.display().to_string(),
            &plain.display().to_string(),
        ],
    );
    let refs = git(&plain, &["for-each-ref", "--format=%(refname)"]);
    assert!(!refs.contains("refs/forge/blobs"), "{refs}");
    let rock = git(
        &gd,
        &[
            "rev-parse",
            &format!("refs/forge/blobs:{}", Blake3::of(ROCK).to_hex()),
        ],
    );
    assert!(
        !common::git_ok(&plain, &["cat-file", "-e", rock.trim()]),
        "a plain clone does not hold the binary"
    );
}

#[test]
fn a_lost_index_is_rebuilt_from_the_history() {
    let d = tmp("lost-index");
    let mut g = GitStore::open_folder(d.join("proj"), "ada").unwrap_or_else(|e| panic!("{e}"));
    script(&mut g).unwrap_or_else(|e| panic!("{e}"));
    let want = snapshot(&g).unwrap_or_else(|e| panic!("{e}"));
    let index = g.git_dir().join("forge-index");
    drop(g);
    std::fs::remove_file(&index).unwrap_or_else(|e| panic!("{e}"));
    let g = GitStore::open_folder(d.join("proj"), "ada").unwrap_or_else(|e| panic!("{e}"));
    let got = snapshot(&g).unwrap_or_else(|e| panic!("{e}"));
    assert!(differences(&want, &got).is_empty());
    // A torn last line is skipped, not fatal.
    let mut text = std::fs::read_to_string(&index).unwrap_or_default();
    text.push_str("b 12ab");
    std::fs::write(&index, text).unwrap_or_else(|e| panic!("{e}"));
    drop(g);
    let g = GitStore::open_folder(d.join("proj"), "ada").unwrap_or_else(|e| panic!("{e}"));
    assert!(snapshot(&g).is_ok());
}

#[test]
fn push_clone_and_pull_through_a_bare_repository_keep_ids_and_git_history() {
    let d = tmp("bare");
    let bare = d.join("nas").join("orbits.git");
    let url = format!("git-file:{}", bare.display());
    let mut ada = GitStore::open_folder(d.join("ada"), "ada").unwrap_or_else(|e| panic!("{e}"));
    let ids = script(&mut ada).unwrap_or_else(|e| panic!("{e}"));
    // Push: a view of the (not yet existing) remote, the revisions replayed, published.
    let mut remote =
        GitStore::open_remote(&url, "ada", &opts(&d.join("a"))).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(remote.head().ok().flatten(), None, "an empty remote");
    assert_eq!(
        push(&ada, &mut remote).unwrap_or_else(|e| panic!("{e}")),
        ids
    );
    git(&bare, &["fsck", "--strict", "--no-dangling"]);
    // The same history is the same Git history: the remote's commits are ada's.
    assert_eq!(
        git(&bare, &["rev-parse", "refs/heads/main"]),
        git(ada.git_dir(), &["rev-parse", "refs/heads/main"])
    );
    assert_eq!(
        git(&bare, &["rev-parse", "refs/forge/blobs"]),
        git(ada.git_dir(), &["rev-parse", "refs/forge/blobs"])
    );
    // Clone: a fresh view (its own cache) fetches both refs; bob's project pulls from it.
    let view =
        GitStore::open_remote(&url, "bob", &opts(&d.join("b"))).unwrap_or_else(|e| panic!("{e}"));
    let mut bob = MemoryStore::new("bob");
    assert_eq!(push(&view, &mut bob).unwrap_or_else(|e| panic!("{e}")), ids);
    assert_eq!(
        bob.read(&p("assets/rock.png")).ok().as_deref(),
        Some(ROCK),
        "the binary came through the blobs chain"
    );
    let want = snapshot(&ada).unwrap_or_else(|e| panic!("{e}"));
    let got = snapshot(&view).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(want.revs, got.revs, "every revision, tree and log");
    assert_eq!(
        got.files, want.revs[0].tree,
        "a view's working files are the remote head's"
    );
    let times: Vec<u64> = view
        .history(RevRange::all())
        .unwrap_or_default()
        .iter()
        .map(|r| r.at_ms)
        .collect();
    let orig: Vec<u64> = ada
        .history(RevRange::all())
        .unwrap_or_default()
        .iter()
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(times, orig, "a pushed revision keeps when it was made");

    // Bob works and pushes a new binary; ada pulls it.
    bob.write(&p("assets/moon.png"), Bytes::from_static(b"\0moon"))
        .unwrap_or_else(|e| panic!("{e}"));
    let r3 = bob
        .commit("bob's moon", &[])
        .unwrap_or_else(|e| panic!("{e}"));
    let mut view =
        GitStore::open_remote(&url, "bob", &opts(&d.join("b"))).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        push(&bob, &mut view).unwrap_or_else(|e| panic!("{e}")),
        vec![r3]
    );
    assert_eq!(
        git(&bare, &["rev-list", "--count", "refs/forge/blobs"]).trim(),
        "2"
    );
    let fresh =
        GitStore::open_remote(&url, "ada", &opts(&d.join("a"))).unwrap_or_else(|e| panic!("{e}"));
    let mut ada2 = MemoryStore::new("ada");
    push(&fresh, &mut ada2).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(ada2.head().ok().flatten(), Some(r3));
    assert_eq!(
        ada2.read(&p("assets/moon.png")).ok().as_deref(),
        Some(&b"\0moon"[..])
    );
}

#[test]
fn a_push_never_overwrites_a_remote_that_moved() {
    let d = tmp("moved");
    let bare = d.join("remote.git");
    let url = format!("git-file:{}", bare.display());
    let mut base = MemoryStore::new("ada");
    base.write(&p("a.ron"), Bytes::from_static(b"A(1)"))
        .unwrap_or_else(|e| panic!("{e}"));
    base.commit("base", &[]).unwrap_or_else(|e| panic!("{e}"));
    let mut v =
        GitStore::open_remote(&url, "ada", &opts(&d.join("x"))).unwrap_or_else(|e| panic!("{e}"));
    push(&base, &mut v).unwrap_or_else(|e| panic!("{e}"));
    // Two views read the same remote; both commit on top of it.
    let mut v1 =
        GitStore::open_remote(&url, "ada", &opts(&d.join("v1"))).unwrap_or_else(|e| panic!("{e}"));
    let mut v2 =
        GitStore::open_remote(&url, "bob", &opts(&d.join("v2"))).unwrap_or_else(|e| panic!("{e}"));
    v1.write(&p("a.ron"), Bytes::from_static(b"A(2)"))
        .unwrap_or_else(|e| panic!("{e}"));
    v1.commit("ada's", &[]).unwrap_or_else(|e| panic!("{e}"));
    v2.write(&p("b.ron"), Bytes::from_static(b"\0binary"))
        .unwrap_or_else(|e| panic!("{e}"));
    v2.commit("bob's", &[]).unwrap_or_else(|e| panic!("{e}"));
    v1.publish().unwrap_or_else(|e| panic!("{e}"));
    let before = git(&bare, &["rev-parse", "refs/heads/main"]);
    let e = v2.publish().err();
    assert!(matches!(e, Some(StoreError::RemoteMoved { .. })), "{e:?}");
    assert_eq!(
        git(&bare, &["rev-parse", "refs/heads/main"]),
        before,
        "nothing moved"
    );
    assert!(
        e.map(|e| e.to_string())
            .is_some_and(|m| m.starts_with("STORE-0011") && m.contains("pull")),
    );
}

#[test]
fn a_foreign_commit_is_named_not_misread() {
    let d = tmp("foreign");
    let bare = d.join("plain.git");
    // A repository made with plain git: its commit has no Forge metadata.
    git(
        &d,
        &["init", "--quiet", "--bare", &bare.display().to_string()],
    );
    let work = d.join("w");
    git(&d, &["init", "--quiet", &work.display().to_string()]);
    std::fs::write(work.join("notes.txt"), "hello\n").unwrap_or_else(|e| panic!("{e}"));
    git(&work, &["add", "."]);
    git(
        &work,
        &[
            "-c",
            "user.name=x",
            "-c",
            "user.email=x@y",
            "commit",
            "--quiet",
            "-m",
            "plain",
        ],
    );
    git(
        &work,
        &[
            "push",
            "--quiet",
            &bare.display().to_string(),
            "HEAD:refs/heads/main",
        ],
    );
    let e = GitStore::open_remote(&format!("git-file:{}", bare.display()), "ada", &opts(&d))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(e.contains("not made by Forge"), "{e}");
}

#[test]
fn a_view_reads_lazily_and_writes_nothing_until_published() {
    let d = tmp("lazy");
    let bare = d.join("r.git");
    let url = format!("git-file:{}", bare.display());
    let mut src = MemoryStore::new("ada");
    let big = vec![7u8; 3 << 20];
    src.write(&p("big.bin"), Bytes::from(big.clone()))
        .unwrap_or_else(|e| panic!("{e}"));
    src.commit("big", &[]).unwrap_or_else(|e| panic!("{e}"));
    let mut v =
        GitStore::open_remote(&url, "ada", &opts(&d.join("c"))).unwrap_or_else(|e| panic!("{e}"));
    push(&src, &mut v).unwrap_or_else(|e| panic!("{e}"));
    let mut v =
        GitStore::open_remote(&url, "ada", &opts(&d.join("c"))).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v.size(&p("big.bin")).ok().flatten(), Some(big.len() as u64));
    v.write(&p("x.ron"), Bytes::from_static(b"X"))
        .unwrap_or_else(|e| panic!("{e}"));
    v.commit("local only", &[])
        .unwrap_or_else(|e| panic!("{e}"));
    let before = git(&bare, &["rev-parse", "refs/heads/main"]);
    assert_eq!(git(&bare, &["rev-parse", "refs/heads/main"]), before);
    v.publish().unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(git(&bare, &["rev-parse", "refs/heads/main"]), before);
}

/// The guarded property of Git remotes (gate row `C-store-git-remote`): every revision a
/// push reported is in the remote's history afterwards — nothing a push or a racing push
/// does can drop one. The revisions of `pushed` the remote no longer has.
fn lost(
    url: &str,
    pushed: &[forge_store::RevId],
    cache: &std::path::Path,
) -> Vec<forge_store::RevId> {
    let view =
        GitStore::open_remote(url, "auditor", &opts(cache)).unwrap_or_else(|e| panic!("{e}"));
    let have: Vec<forge_store::RevId> = view
        .history(RevRange::all())
        .unwrap_or_default()
        .iter()
        .map(|r| r.id)
        .collect();
    pushed
        .iter()
        .filter(|r| !have.contains(r))
        .copied()
        .collect()
}

/// Two editors race: both commit on the same head, both publish.
fn race(url: &str, d: &std::path::Path) -> (forge_store::RevId, GitStore) {
    let mut base = MemoryStore::new("ada");
    base.write(&p("a.ron"), Bytes::from_static(b"A(1)"))
        .unwrap_or_else(|e| panic!("{e}"));
    base.commit("base", &[]).unwrap_or_else(|e| panic!("{e}"));
    let mut v =
        GitStore::open_remote(url, "ada", &opts(&d.join("x"))).unwrap_or_else(|e| panic!("{e}"));
    push(&base, &mut v).unwrap_or_else(|e| panic!("{e}"));
    let mut v1 =
        GitStore::open_remote(url, "ada", &opts(&d.join("v1"))).unwrap_or_else(|e| panic!("{e}"));
    let mut v2 =
        GitStore::open_remote(url, "bob", &opts(&d.join("v2"))).unwrap_or_else(|e| panic!("{e}"));
    v1.write(&p("a.ron"), Bytes::from_static(b"A(2)"))
        .unwrap_or_else(|e| panic!("{e}"));
    let r1 = v1.commit("ada's", &[]).unwrap_or_else(|e| panic!("{e}"));
    v2.write(&p("b.ron"), Bytes::from_static(b"B"))
        .unwrap_or_else(|e| panic!("{e}"));
    v2.commit("bob's", &[]).unwrap_or_else(|e| panic!("{e}"));
    v1.publish().unwrap_or_else(|e| panic!("{e}"));
    (r1, v2)
}

#[test]
fn racing_pushes_lose_no_revision() {
    let d = tmp("race");
    let url = format!("git-file:{}", d.join("remote.git").display());
    let (r1, mut v2) = race(&url, &d);
    assert!(matches!(v2.publish(), Err(StoreError::RemoteMoved { .. })));
    assert!(lost(&url, &[r1], &d.join("audit")).is_empty());
}

#[test]
fn positive_control_a_forced_push_loses_a_revision_and_is_caught() {
    let d = tmp("race-forced");
    let bare = d.join("remote.git");
    let url = format!("git-file:{}", bare.display());
    let (r1, v2) = race(&url, &d);
    // What a transport without the compare-and-swap does: the second editor's head is
    // forced over the remote's (real git, `+` refspec).
    let mirror = v2.git_dir().display().to_string();
    git(
        &bare,
        &[
            "fetch",
            "--quiet",
            &mirror,
            "+refs/heads/main:refs/heads/main",
        ],
    );
    assert_eq!(
        lost(&url, &[r1], &d.join("audit")),
        vec![r1],
        "a forced push dropped ada's revision and the check names it"
    );
}
