//! Import performance, recorded (M2-6). Prints one line per measurement; the numbers go in
//! ADR 0015 and Ch.8 §8.9. Timing is not asserted (a loaded CI machine is not a regression);
//! the **work** is: a fresh record runs no importer, a quiet poll reads nothing.
//!
//! `cargo test -p forge-asset --release --test test_import_perf -- --nocapture` for the
//! numbers worth quoting. Each recording runs alone (`forge_trace::timed::run_timed_alone`:
//! its own process, High priority, the machine-wide timed lock; WP-19): the poll interval is
//! derived from a measured poll, and the numbers are quoted.

mod common;

use std::time::Instant;

use common::{WAIT, open, p, put_scene};
use forge_asset::fixture::SceneSpec;
use forge_asset::{AssetServer, MeshAsset, SceneAsset};
use forge_store::{MemoryStore, ProjectStore};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// Run a recording alone and print what it reports.
fn timed(body: impl FnOnce() -> String) {
    let s = forge_trace::timed::run_timed_alone(|| Ok(body())).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("{s}");
}

#[test]
fn import_500_objects_and_record_the_numbers() {
    timed(import_500_objects);
}

fn import_500_objects() -> String {
    let mut out = String::new();
    for (label, grid) in [
        ("500 objects x 25 verts", 4usize),
        ("500 objects x 1089 verts", 32),
    ] {
        let spec = SceneSpec {
            name: "city.gltf".into(),
            objects: 500,
            grid,
            materials: 8,
            textured: true,
            seed: 3,
        };
        let mut s = MemoryStore::new("ada");
        put_scene(&mut s, "levels", &spec);
        let bin_len = s
            .read(&p("levels/city.bin"))
            .map(|b| b.len())
            .unwrap_or_default();
        let mut a = open(Box::new(s));

        let t = Instant::now();
        let id = common::import(&mut a, "levels/city.gltf");
        let cold = ms(t);
        assert_eq!(a.import_runs(), 1);

        let t = Instant::now();
        let mut ev = Vec::new();
        a.reimport(&p("levels/city.gltf"), &mut ev)
            .expect("reimport");
        let warm = ms(t);
        assert_eq!(a.import_runs(), 1, "a fresh record runs no importer");

        let t = Instant::now();
        let scene = a.load::<SceneAsset>(id).wait(WAIT).expect("scene");
        let handles: Vec<_> = scene
            .nodes
            .iter()
            .filter_map(|n| n.mesh)
            .map(|m| a.load::<MeshAsset>(m))
            .collect();
        for h in &handles {
            h.wait(WAIT).expect("mesh");
        }
        let load_all = ms(t);

        // The first poll sees the sidecar the import wrote (and recognises it); the second
        // is a quiet project's steady state.
        let t = Instant::now();
        assert!(a.poll().expect("poll").is_empty());
        let first_poll = ms(t);
        let t = Instant::now();
        let quiet = a.poll().expect("poll");
        let poll = ms(t);
        assert!(quiet.is_empty());

        let t = Instant::now();
        a.thumbnail(id, 128).wait(WAIT).expect("thumbnail");
        let thumb = ms(t);

        let vfs = a.vfs().clone();
        drop(handles);
        drop(a);
        let t = Instant::now();
        let (b, _) = AssetServer::open_vfs(vfs, &forge_importers::asset_extensions().expect("x"))
            .expect("reopen");
        let reopen = ms(t);
        assert_eq!(
            b.import_runs(),
            0,
            "reopening a fresh project runs no importer"
        );

        out += &format!(
            "import-perf [{label}, bin {:.1} MiB, {} build]: cold import {cold:.1} ms | \
             fresh reimport {warm:.2} ms | load 500 meshes {load_all:.1} ms | first poll \
             {first_poll:.2} ms | quiet poll {poll:.3} ms | 128px scene thumbnail {thumb:.1} ms | reopen {reopen:.1} ms\n",
            bin_len as f64 / 1_048_576.0,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
        );
    }
    out
}

/// The poll on a real folder: the 500-object scene imported into a `LocalFs` project that
/// also holds 10 000 other files (release; a debug build — the gate run — uses 1 000 files and
/// a light scene, and only the work is asserted either way). A quiet poll runs no importer and returns nothing; its time
/// is one directory walk (`forge-store/tests/test_local_stamps.rs` guards that it reads no
/// file) and sets the editor's poll interval.
#[test]
fn a_quiet_poll_on_a_real_10k_file_project_is_recorded() {
    timed(quiet_poll_on_a_real_project);
}

fn quiet_poll_on_a_real_project() -> String {
    let (files, grid) = if cfg!(debug_assertions) {
        (1_000u32, 4)
    } else {
        (10_000, 32)
    };
    let dir = common::tmp_dir("perf-localfs");
    let mut s = forge_store::LocalFs::open(&dir, "ada").expect("local fs");
    let spec = SceneSpec {
        name: "city.gltf".into(),
        objects: 500,
        grid,
        materials: 8,
        textured: true,
        seed: 3,
    };
    put_scene(&mut s, "levels", &spec);
    // The filler is written straight to disk (a store write fsyncs each file: minutes).
    for i in 0..files {
        let d = dir.join("content").join(format!("d{:03}", i / 100));
        if i % 100 == 0 {
            std::fs::create_dir_all(&d).expect("dir");
        }
        std::fs::write(d.join(format!("f{i:05}.dat")), i.to_le_bytes()).expect("write");
    }
    let mut a = open(Box::new(s));
    common::import(&mut a, "levels/city.gltf");
    // Past every racy window, so the steady state is measured.
    std::thread::sleep(std::time::Duration::from_millis(2100));
    assert!(a.poll().expect("poll").is_empty());
    let runs = a.import_runs();
    let mut times = Vec::new();
    for _ in 0..9 {
        let t = Instant::now();
        assert!(a.poll().expect("poll").is_empty());
        times.push(ms(t));
    }
    assert_eq!(a.import_runs(), runs, "a quiet poll runs no importer");
    times.sort_by(f64::total_cmp);
    let interval = a.poll_interval();
    assert!(
        (std::time::Duration::from_millis(250)..=std::time::Duration::from_secs(2))
            .contains(&interval)
    );
    let line = format!(
        "import-perf [LocalFs, 10 000 files + 500-object scene, {} build]: quiet poll median \
         {:.1} ms (min {:.1}, max {:.1}) -> poll interval {interval:?}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        times[times.len() / 2],
        times[0],
        times[times.len() - 1],
    );
    drop(a);
    let _ = std::fs::remove_dir_all(&dir);
    line
}
