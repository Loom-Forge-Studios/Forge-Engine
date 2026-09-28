//! `LocalFs` behaviour a user depends on: the project is an ordinary folder, writes are
//! atomic, corruption is detected rather than handed back, locks hold across two editors on
//! one folder, history survives a reopen, and non-portable paths are refused.

use std::fs;
use std::path::PathBuf;

use forge_store::{Bytes, LocalFs, ProjectStore, RevRange, StorePath};

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("forge-store")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    d
}

fn p(s: &str) -> StorePath {
    StorePath::new(s).expect("valid path")
}

#[test]
fn the_project_is_an_ordinary_folder_and_history_survives_a_reopen() {
    let root = dir("ordinary");
    let rev = {
        let mut s = LocalFs::open(&root, "ada").expect("open");
        s.write(&p("scenes/main.ron"), Bytes::from_static(b"Scene()"))
            .expect("write");
        s.write(&p("assets/rock.bin"), Bytes::from_static(&[0, 1, 2, 255]))
            .expect("write");
        s.commit("first", &[]).expect("commit")
    };
    // A user (or git, or a diff tool) sees plain files.
    assert_eq!(
        fs::read(root.join("scenes").join("main.ron")).expect("plain file"),
        b"Scene()"
    );
    // A second editor session on the folder sees the same project and history.
    let s = LocalFs::open(&root, "ada").expect("reopen");
    assert_eq!(s.head().expect("head"), Some(rev));
    assert_eq!(
        s.list().expect("list"),
        [p("assets/rock.bin"), p("scenes/main.ron")]
    );
    let h = s.history(RevRange::all()).expect("history");
    assert_eq!((h.len(), h[0].message.as_str()), (1, "first"));
    // The store's own `.forge/` folder is not project content.
    assert!(root.join(".forge").is_dir());
    assert!(
        s.list()
            .expect("list")
            .iter()
            .all(|f| !f.as_str().starts_with(".forge"))
    );
}

#[test]
fn a_tampered_blob_is_refused_not_returned() {
    let root = dir("tamper");
    let mut s = LocalFs::open(&root, "ada").expect("open");
    let h = s
        .blob_put(Bytes::from_static(b"texture bytes"))
        .expect("put");
    assert_eq!(
        s.blob_get(h).expect("get"),
        Bytes::from_static(b"texture bytes")
    );
    let hex = h.to_hex();
    let file = root
        .join(".forge")
        .join("blobs")
        .join(&hex[..2])
        .join(&hex[2..]);
    fs::write(&file, b"texture bytez").expect("tamper");
    let e = s.blob_get(h).expect_err("corrupt");
    assert_eq!(e.code().as_str(), "STORE-0004");
    // A revision record edited by hand no longer matches its id either.
    s.write(&p("a.ron"), Bytes::from_static(b"A"))
        .expect("write");
    let rev = s.commit("msg", &[]).expect("commit");
    let rec = root.join(".forge").join("revs").join(format!("{rev}.ron"));
    let text = fs::read_to_string(&rec).expect("record");
    fs::write(&rec, text.replace("msg", "forged")).expect("tamper");
    assert_eq!(
        s.history(RevRange::all()).map_err(|e| e.code().as_str()),
        Err("STORE-0004")
    );
}

#[test]
fn locks_hold_across_two_editors_on_one_folder() {
    let root = dir("locks");
    let mut ada = LocalFs::open(&root, "ada").expect("open");
    let mut bob = LocalFs::open(&root, "bob").expect("open");
    let tex = p("assets/hero.psd");
    ada.write(&tex, Bytes::from_static(b"v1")).expect("write");
    let lock = ada.lock(&tex).expect("ada locks");
    assert_eq!(ada.lock(&tex).expect("idempotent for the holder"), lock);
    let e = bob.lock(&tex).expect_err("bob cannot");
    assert!(e.to_string().contains("locked by ada"), "{e}");
    assert_eq!(
        bob.write(&tex, Bytes::from_static(b"bob's"))
            .map_err(|e| e.code().as_str()),
        Err("STORE-0005")
    );
    assert_eq!(
        bob.delete(&tex).map_err(|e| e.code().as_str()),
        Err("STORE-0005")
    );
    assert_eq!(
        bob.unlock(&lock).map_err(|e| e.code().as_str()),
        Err("STORE-0006")
    );
    assert_eq!(bob.locks().expect("locks"), std::slice::from_ref(&lock));
    ada.write(&tex, Bytes::from_static(b"v2"))
        .expect("the holder writes");
    ada.unlock(&lock).expect("unlock");
    bob.write(&tex, Bytes::from_static(b"v3"))
        .expect("released");
    assert_eq!(ada.read(&tex).expect("read"), Bytes::from_static(b"v3"));
}

#[test]
fn case_collisions_and_non_portable_paths_are_refused() {
    let root = dir("case");
    let mut s = LocalFs::open(&root, "ada").expect("open");
    s.write(&p("Textures/rock.png"), Bytes::from_static(b"r"))
        .expect("write");
    for clash in ["textures/grass.png", "Textures/ROCK.png"] {
        assert_eq!(
            s.write(&p(clash), Bytes::from_static(b"x"))
                .map_err(|e| e.code().as_str()),
            Err("STORE-0007"),
            "{clash}"
        );
    }
    s.write(&p("Textures/grass.png"), Bytes::from_static(b"g"))
        .expect("same spelling");
    s.write(&p("Textures/rock.png"), Bytes::from_static(b"r2"))
        .expect("overwrite");
    for bad in ["aux.png", "a:b", "x/../y"] {
        assert!(StorePath::new(bad).is_err(), "{bad}");
    }
}

#[test]
fn deleting_the_last_file_removes_its_empty_directories() {
    let root = dir("delete");
    let mut s = LocalFs::open(&root, "ada").expect("open");
    s.write(&p("a/b/c.ron"), Bytes::from_static(b"c"))
        .expect("write");
    s.delete(&p("a/b/c.ron")).expect("delete");
    assert!(!root.join("a").exists());
    assert_eq!(
        s.delete(&p("a/b/c.ron")).map_err(|e| e.code().as_str()),
        Err("STORE-0002")
    );
    assert!(root.join(".forge").exists(), "the store's own folder stays");
}

#[test]
fn a_commit_rehashes_only_changed_files_and_sees_same_size_rewrites() {
    let root = dir("rehash");
    let mut s = LocalFs::open(&root, "ada").expect("open");
    s.write(&p("a.ron"), Bytes::from_static(b"AAAA"))
        .expect("write");
    let r1 = s.commit("one", &[]).expect("commit");
    // Same length, new content, within the same timestamp tick: must be seen.
    s.write(&p("a.ron"), Bytes::from_static(b"BBBB"))
        .expect("write");
    let r2 = s.commit("two", &[]).expect("commit");
    let t1 = s.tree(&r1).expect("tree");
    let t2 = s.tree(&r2).expect("tree");
    assert_ne!(
        t1.entries[0].1, t2.entries[0].1,
        "the rewrite was committed"
    );
    assert_eq!(
        s.blob_get(t2.entries[0].1).expect("snapshot"),
        Bytes::from_static(b"BBBB")
    );
}
