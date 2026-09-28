//! M2-13 hot reload: a WASM plugin's code changes on disk and the **installed** item runs
//! the new code — same registry entry, same owner, no reload of the plugin set; a broken
//! save keeps the old code; a quiet poll reads nothing; a manifest change asks for a
//! reload through the loader.
//!
//! Positive control: `positive_control_without_a_poll_the_old_code_answers` — the file on
//! disk changes but nobody polls, and the command must still run the old code, so the
//! reload assertions observe the swap and not merely the file.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use forge_cmd::Value;
use forge_plugin::SharedGrants;
use forge_plugin::points::Command;
use forge_wasm::{PluginWatcher, ReloadEvent, WasmHost};

fn plan(version: i64) -> String {
    plan_plugin(&format!(
        r#"{{"ops":[{{"op":"set_setting","key":"version","value":{{"Int":{version}}}}}]}}"#
    ))
}

const MANIFEST: &str = r#"Plugin(id: "com.example.live", version: "0.1.0", engine: "^0.1",
    kind: Wasm, provides: [Command("example.live")], capabilities: [Command(Ordinary)])"#;

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("forge-wasm-hot-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(d.join("plugin.ron"), MANIFEST).unwrap_or_else(|e| panic!("{e}"));
        Self(d)
    }

    /// Write the code file, making sure its size or mtime differs from the last write even
    /// on a coarse clock (the watcher compares metadata before reading).
    fn write_code(&self, text: &str) {
        let p = self.0.join("plugin.wat");
        let before = std::fs::metadata(&p).ok();
        std::fs::write(&p, text).unwrap_or_else(|e| panic!("{e}"));
        if let Some(b) = before {
            let after = std::fs::metadata(&p).unwrap_or_else(|e| panic!("{e}"));
            if after.len() == b.len() && after.modified().ok() == b.modified().ok() {
                // Same size within the clock tick: pad so the change is visible.
                std::fs::write(&p, format!("{text}\n;; again")).unwrap_or_else(|e| panic!("{e}"));
            }
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn version(bus: &forge_cmd::Bus) -> Option<Value> {
    bus.project().setting("version").cloned()
}

#[test]
fn changed_code_runs_under_the_installed_item_and_a_broken_save_keeps_the_old() {
    let dir = Dir::new("swap");
    dir.write_code(&plan(1));
    let grants = SharedGrants::new();
    grants.grant(who("com.example.live"), ORDINARY);
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let plugin = host.load_dir(dir.path()).unwrap_or_else(|e| panic!("{e}"));
    let (mut bus, ext) = bus_with(&[], &[&plugin], &grants).unwrap_or_else(|e| panic!("{e}"));
    let mut watcher = PluginWatcher::new();
    watcher
        .watch(dir.path(), &plugin)
        .unwrap_or_else(|e| panic!("{e}"));

    run(&mut bus, "example.live", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(version(&bus), Some(Value::Int(1)));

    // Quiet: nothing read, nothing reported.
    for _ in 0..3 {
        assert!(watcher.poll().is_empty());
        assert_eq!(watcher.last_poll_reads(), 0);
    }

    // New code: swapped under the installed command (the bus's handler was registered once,
    // before the change).
    dir.write_code(&plan(2));
    let events = watcher.poll();
    assert_eq!(
        events,
        vec![ReloadEvent::Reloaded {
            plugin: plugin.id().clone(),
            generation: 2
        }]
    );
    run(&mut bus, "example.live", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(version(&bus), Some(Value::Int(2)));
    let prov = ext
        .registry::<Command>()
        .and_then(|r| r.provenance("example.live"))
        .unwrap_or_else(|| panic!("still registered"));
    assert_eq!(prov.owner.as_str(), "com.example.live");

    // A broken save: refused, and the old code keeps serving.
    dir.write_code("(component (core module $m (func (export \"call\"");
    let events = watcher.poll();
    assert!(
        matches!(events.as_slice(), [ReloadEvent::Failed { error, .. }] if error.code().as_str() == "WASM-0001"),
        "{events:?}"
    );
    bus.clear_history();
    run(&mut bus, "example.live", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(plugin.generation(), 2);

    // Fixed again: the next poll picks it up.
    dir.write_code(&plan(3));
    assert!(matches!(
        watcher.poll().as_slice(),
        [ReloadEvent::Reloaded { generation: 3, .. }]
    ));
    run(&mut bus, "example.live", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(version(&bus), Some(Value::Int(3)));

    // The same bytes rewritten: read once to compare, not a reload.
    let p = dir.path().join("plugin.wat");
    let same = std::fs::read(&p).unwrap_or_else(|e| panic!("{e}"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&p, &same).unwrap_or_else(|e| panic!("{e}"));
    assert!(watcher.poll().is_empty());
    assert_eq!(plugin.generation(), 3);
}

#[test]
fn a_changed_manifest_asks_for_a_reload_through_the_loader() {
    let dir = Dir::new("manifest");
    dir.write_code(&plan(1));
    let grants = SharedGrants::new();
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let plugin = host.load_dir(dir.path()).unwrap_or_else(|e| panic!("{e}"));
    let mut watcher = PluginWatcher::new();
    watcher
        .watch(dir.path(), &plugin)
        .unwrap_or_else(|e| panic!("{e}"));

    // The plugin now also claims to replace something: declarations never change under the
    // loader's conflict check.
    std::fs::write(
        dir.path().join("plugin.ron"),
        MANIFEST.replace(
            "provides:",
            "replaces: [Command(\"forge.other\")], provides:",
        ),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let events = watcher.poll();
    assert!(
        matches!(events.as_slice(), [ReloadEvent::NeedsLoad { error, .. }] if error.code().as_str() == "WASM-0011"),
        "{events:?}"
    );
    // Reported once; the host reloads and re-watches.
    assert!(watcher.poll().is_empty());
    assert_eq!(plugin.generation(), 1, "no code was swapped");
}

#[test]
fn positive_control_without_a_poll_the_old_code_answers() {
    let dir = Dir::new("control");
    dir.write_code(&plan(1));
    let grants = SharedGrants::new();
    grants.grant(who("com.example.live"), ORDINARY);
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let plugin = host.load_dir(dir.path()).unwrap_or_else(|e| panic!("{e}"));
    let (mut bus, _) = bus_with(&[], &[&plugin], &grants).unwrap_or_else(|e| panic!("{e}"));
    dir.write_code(&plan(2));
    run(&mut bus, "example.live", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(
        version(&bus),
        Some(Value::Int(1)),
        "the file changed but nothing reloaded it: the old code must still answer"
    );
}

// ---- WP-36: what a host vetted is what runs --------------------------------------------
//
// A host that vets a plugin by its bytes (the editor's project trust) compiles from the one
// read it vetted (`PluginFiles` → `WasmHost::load_files`), and the watcher shows each changed
// code file to the host's vet before compiling it (`PluginWatcher::poll_vetted`): a refusal
// keeps the old code and leaves the change unstamped, so allowing it later reloads it.

#[test]
fn a_plugin_loaded_from_files_runs_the_bytes_read_not_a_later_write() {
    let dir = Dir::new("files");
    dir.write_code(&plan(1));
    let grants = SharedGrants::new();
    grants.grant(who("com.example.live"), ORDINARY);
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let files = forge_wasm::PluginFiles::read(dir.path()).unwrap_or_else(|e| panic!("{e}"));
    // A writer swaps the code between the read (the vet) and the load.
    dir.write_code(&plan(9));
    let plugin = host.load_files(&files).unwrap_or_else(|e| panic!("{e}"));
    let (mut bus, _) = bus_with(&[], &[&plugin], &grants).unwrap_or_else(|e| panic!("{e}"));
    run(&mut bus, "example.live", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(version(&bus), Some(Value::Int(1)), "the vetted bytes run");
    // The watcher starts from the bytes compiled: the later write is a change it sees.
    let mut watcher = PluginWatcher::new();
    watcher
        .watch(dir.path(), &plugin)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(matches!(
        watcher.poll().as_slice(),
        [ReloadEvent::Reloaded { generation: 2, .. }]
    ));
    run(&mut bus, "example.live", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(version(&bus), Some(Value::Int(9)));
}

#[test]
fn a_vetted_poll_shows_the_bytes_it_compiles_and_a_refusal_keeps_the_old_code() {
    let dir = Dir::new("vetted");
    dir.write_code(&plan(1));
    let grants = SharedGrants::new();
    grants.grant(who("com.example.live"), ORDINARY);
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let plugin = host.load_dir(dir.path()).unwrap_or_else(|e| panic!("{e}"));
    let (mut bus, _) = bus_with(&[], &[&plugin], &grants).unwrap_or_else(|e| panic!("{e}"));
    let mut watcher = PluginWatcher::new();
    watcher
        .watch(dir.path(), &plugin)
        .unwrap_or_else(|e| panic!("{e}"));
    dir.write_code(&plan(2));
    let mut seen: Vec<Vec<u8>> = Vec::new();
    let events = watcher.poll_vetted(
        |_| true,
        |_, f| {
            assert_eq!(
                f.manifest_bytes(),
                MANIFEST.as_bytes(),
                "the manifest in effect"
            );
            seen.push(f.code().map(|(_, b)| b.to_vec()).unwrap_or_default());
            false
        },
    );
    assert!(
        matches!(events.as_slice(), [ReloadEvent::Held { .. }]),
        "{events:?}"
    );
    assert_eq!(
        seen,
        vec![plan(2).into_bytes()],
        "the vet saw the new bytes"
    );
    run(&mut bus, "example.live", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(
        version(&bus),
        Some(Value::Int(1)),
        "refused: the old code serves"
    );
    // Unstamped: the next poll that allows it reloads it.
    assert!(matches!(
        watcher.poll().as_slice(),
        [ReloadEvent::Reloaded { generation: 2, .. }]
    ));
    run(&mut bus, "example.live", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(version(&bus), Some(Value::Int(2)));
}

// A write landing **after** the vet allowed a change and before the swap (a `git pull` between
// the two) must not run: the watcher compiles the bytes it showed the vet, not a second read.
// The seam is the vet itself: it writes new code into the folder, then allows the change.
// Positive control: `positive_control_a_watcher_that_reads_again_runs_the_later_write`
// (`WatchFaults::reread_after_vet`): the written code then runs.

/// What runs after a vetted poll whose vet writes `plan(9)` before allowing `plan(2)`.
fn written_after_the_vet(tag: &str, faults: forge_wasm::WatchFaults) -> Option<Value> {
    let dir = Dir::new(tag);
    dir.write_code(&plan(1));
    let grants = SharedGrants::new();
    grants.grant(who("com.example.live"), ORDINARY);
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let plugin = host.load_dir(dir.path()).unwrap_or_else(|e| panic!("{e}"));
    let (mut bus, _) = bus_with(&[], &[&plugin], &grants).unwrap_or_else(|e| panic!("{e}"));
    let mut watcher = PluginWatcher::new();
    watcher.faults = faults;
    watcher
        .watch(dir.path(), &plugin)
        .unwrap_or_else(|e| panic!("{e}"));
    dir.write_code(&plan(2));
    let mut vetted: Vec<Vec<u8>> = Vec::new();
    let events = watcher.poll_vetted(
        |_| true,
        |_, f| {
            vetted.push(f.code().map(|(_, b)| b.to_vec()).unwrap_or_default());
            dir.write_code(&plan(9)); // the seam: after the vet looked, before the compile
            true
        },
    );
    assert!(
        matches!(events.as_slice(), [ReloadEvent::Reloaded { .. }]),
        "{events:?}"
    );
    assert_eq!(vetted, vec![plan(2).into_bytes()], "the vet saw the change");
    run(&mut bus, "example.live", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    let ran = version(&bus);
    // The later write is a change of its own: the next poll shows it to the vet.
    let mut seen = 0;
    let _ = watcher.poll_vetted(
        |_| true,
        |_, _| {
            seen += 1;
            false
        },
    );
    assert_eq!(
        seen, 1,
        "the write after the vet is vetted at the next poll"
    );
    ran
}

#[test]
fn a_write_between_the_vet_and_the_compile_never_runs() {
    assert_eq!(
        written_after_the_vet("vet-then-write", forge_wasm::WatchFaults::default()),
        Some(Value::Int(2)),
        "the vetted bytes run, not the ones written after the vet"
    );
}

#[test]
fn positive_control_a_watcher_that_reads_again_runs_the_later_write() {
    assert_eq!(
        written_after_the_vet(
            "vet-then-write-control",
            forge_wasm::WatchFaults {
                reread_after_vet: true
            }
        ),
        Some(Value::Int(9)),
        "a watcher compiling a second read runs the write nobody vetted"
    );
}
