//! `LocalFs::stamps` — the hot-reload poll's cost on a real folder (M2-6, owner rule 2).
//!
//! The editor polls `stamps()` on a timer, so its idle cost is paid for as long as a project
//! is open. What is asserted is the **work**: a quiet poll reads no file (one directory walk,
//! metadata from the directory entries), a just-written large file is hashed at most twice
//! and then never again, `stamps_of` agrees with `stamps`, and a same-size rewrite inside
//! one clock tick is still seen. The wall-clock time of a quiet poll over a large tree is
//! printed and checked against the Ch.8 §8.9 budget in release builds only (a debug build or
//! a loaded CI machine is not a regression signal).
//!
//! `cargo test -p forge-store --release --test test_local_stamps -- --nocapture` prints the
//! numbers; `FORGE_STAMPS_FILES=100000` sizes the tree (default 10 000 in release; 1 000 in a
//! debug build, where only the work is asserted — the gate run stays light for the
//! wall-clock UI budgets running beside it). The two timed tests run alone
//! (`forge_trace::timed::run_timed_alone`: their own process, High priority, the machine-wide
//! timed lock; WP-19), so the release budget is measured without sibling tests or another
//! lane's timed test sharing the disk and the cores.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use forge_store::{Bytes, LocalFs, ProjectStore, StorePath};

fn tmp_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let d = base.join(format!("forge-store-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("tmp dir");
    d
}

fn p(s: &str) -> StorePath {
    StorePath::new(s).expect("path")
}

/// Longer than any racy window (2 s covers whole-second filesystems).
fn settle() {
    std::thread::sleep(Duration::from_millis(2100));
}

/// Quiet-poll budget for 10 000 files, release build (Ch.8 §8.9). Scales linearly.
const BUDGET_PER_10K: Duration = Duration::from_millis(60);

/// Run a timed test alone and print what it reports.
fn timed(body: impl FnOnce() -> String) {
    let s = forge_trace::timed::run_timed_alone(|| Ok(body())).unwrap_or_else(|e| panic!("{e}"));
    println!("{s}");
}

#[test]
fn a_quiet_poll_over_a_large_tree_reads_no_file_and_is_timed() {
    timed(quiet_poll_over_a_large_tree);
}

fn quiet_poll_over_a_large_tree() -> String {
    let n: usize = std::env::var("FORGE_STAMPS_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(debug_assertions) {
            1_000
        } else {
            10_000
        });
    let dir = tmp_dir("stamps-large");
    // 100 files per folder, folders two deep: the shape of a real content tree.
    let t = Instant::now();
    for i in 0..n {
        let d = dir
            .join(format!("pack{:02}", i / 10_000))
            .join(format!("dir{:03}", (i / 100) % 100));
        if i % 100 == 0 {
            std::fs::create_dir_all(&d).expect("dir");
        }
        std::fs::write(d.join(format!("asset{i:06}.bin")), i.to_le_bytes()).expect("write");
    }
    let create = t.elapsed();
    let s = LocalFs::open(&dir, "ada").expect("local fs");
    settle();

    let t = Instant::now();
    let first = s.stamps().expect("stamps");
    let cold = t.elapsed();
    assert_eq!(first.len(), n);
    let hashed = s.stamp_hashed_bytes();
    assert_eq!(hashed, 0, "settled files are stamped from metadata alone");

    let mut times = Vec::new();
    for _ in 0..9 {
        let t = Instant::now();
        let again = s.stamps().expect("stamps");
        times.push(t.elapsed());
        assert_eq!(again, first, "nothing changed");
    }
    times.sort();
    let median = times[times.len() / 2];
    assert_eq!(
        s.stamp_hashed_bytes(),
        hashed,
        "a quiet poll reads no file content"
    );
    let per_file_ns = median.as_nanos() / n as u128;
    let line = format!(
        "LocalFs::stamps, {n} files: create {create:?}, first poll {cold:?}, quiet poll median \
         {median:?} (min {:?}, max {:?}), {per_file_ns} ns/file",
        times[0],
        times[times.len() - 1]
    );
    if !cfg!(debug_assertions) {
        let budget = BUDGET_PER_10K * u32::try_from(n.div_ceil(10_000)).unwrap_or(u32::MAX);
        assert!(
            median <= budget,
            "quiet poll {median:?} over {n} files exceeds the budget {budget:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    line
}

#[test]
fn a_just_written_large_file_is_hashed_at_most_twice() {
    timed(a_just_written_large_file);
}

fn a_just_written_large_file() -> String {
    let dir = tmp_dir("stamps-recent");
    let mut s = LocalFs::open(&dir, "ada").expect("local fs");
    // The property is size-independent; release uses the 28 MiB `.bin` of the perf scene.
    let size: usize = if cfg!(debug_assertions) {
        4 << 20
    } else {
        28 << 20
    };
    s.write(&p("big/terrain.bin"), Bytes::from(vec![7u8; size]))
        .expect("write");
    let first = s.stamps().expect("stamps");
    let after_first = s.stamp_hashed_bytes();
    assert!(
        after_first <= size as u64,
        "hashed once at most while inside the window"
    );
    // Past the window: one more hash (the first began inside it), then never again.
    std::thread::sleep(Duration::from_millis(250));
    let t = Instant::now();
    let mut polls = Vec::new();
    for _ in 0..10 {
        polls.push(s.stamps().expect("stamps"));
    }
    let ten = t.elapsed();
    for pl in &polls {
        assert_eq!(*pl, first, "the stamp does not flip when the window closes");
    }
    let total = s.stamp_hashed_bytes();
    assert!(
        total <= 2 * size as u64,
        "a {size}-byte file was hashed {} times over 11 polls",
        total / size as u64
    );
    let t2 = Instant::now();
    for _ in 0..10 {
        s.stamps().expect("stamps");
    }
    let ten_more = t2.elapsed();
    assert_eq!(
        s.stamp_hashed_bytes(),
        total,
        "once hashed after its window, a file is never read again"
    );
    let _ = std::fs::remove_dir_all(&dir);
    format!(
        "{} MiB just written: 10 polls {ten:?}, then 10 quiet polls {ten_more:?}",
        size >> 20
    )
}

#[test]
fn a_same_size_rewrite_inside_one_tick_is_seen() {
    let dir = tmp_dir("stamps-racy");
    let mut s = LocalFs::open(&dir, "ada").expect("local fs");
    let path = p("a/rock.bin");
    let abs = dir.join("a").join("rock.bin");
    let mtime = |f: &std::path::Path| {
        std::fs::metadata(f)
            .and_then(|m| m.modified())
            .expect("mtime")
    };
    s.write(&path, Bytes::from_static(b"aaaa")).expect("write");
    let tick = mtime(&abs);
    let one = s.stamps().expect("stamps");
    // Build the collision exactly rather than hoping for it: the rewrite gets the same size
    // and the same mtime as the first write (on NTFS two back-to-back writes usually land on
    // different 100 ns stamps, so without this the case would never occur here).
    s.write(&path, Bytes::from_static(b"bbbb")).expect("write");
    std::fs::File::options()
        .write(true)
        .open(&abs)
        .and_then(|f| f.set_modified(tick))
        .expect("set mtime");
    assert_eq!(
        std::fs::metadata(&abs).expect("metadata").len(),
        4,
        "fixture: same size"
    );
    assert_eq!(mtime(&abs), tick, "fixture: same mtime");
    let two = s.stamps().expect("stamps");
    assert_ne!(
        one, two,
        "the content changed under an identical size and mtime"
    );
    std::thread::sleep(Duration::from_millis(250));
    let three = s.stamps().expect("stamps");
    assert_eq!(two, three, "no change after the window closes");
    s.write(&path, Bytes::from_static(b"cccc")).expect("write");
    assert_ne!(
        s.stamps().expect("stamps"),
        three,
        "a later rewrite is seen"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stamps_of_agrees_with_stamps() {
    let dir = tmp_dir("stamps-of");
    let mut s = LocalFs::open(&dir, "ada").expect("local fs");
    for f in ["a/one.txt", "a/two.txt", "b/c/three.txt", "top.txt"] {
        s.write(&p(f), Bytes::from(f.as_bytes().to_vec()))
            .expect("write");
    }
    let want = [
        p("top.txt"),
        p("b/c/three.txt"),
        p("a/one.txt"),
        p("a/missing.txt"),
        p("gone/x.txt"),
        p("a/one.txt"),
    ];
    for _ in 0..2 {
        let all = s.stamps().expect("stamps");
        let some = s.stamps_of(&want).expect("stamps_of");
        let expect: Vec<_> = all
            .iter()
            .filter(|(q, _)| want.contains(q))
            .cloned()
            .collect();
        assert_eq!(some, expect);
        assert_eq!(s.size(&p("a/one.txt")).expect("size"), Some(9));
        assert_eq!(s.size(&p("a/missing.txt")).expect("size"), None);
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
