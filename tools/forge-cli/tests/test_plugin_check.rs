//! `forge plugin check` end to end (Ch.32, WP-15): a WASM plugin directory written in text is
//! loaded by the real `forge` binary through the WASM host and the ordinary loader, reports
//! what it installs and what it requests, runs its command only once `Command(Ordinary)` is
//! granted, and a plugin whose import its manifest does not request does not load.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn forge(args: &[&str]) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_forge"))
        .args(args)
        .output()
        .unwrap();
    (
        o.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    )
}

fn plugin_dir(tag: &str, manifest: &str, code: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("plugin-check-{tag}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("plugin.ron"), manifest).unwrap();
    std::fs::write(d.join("plugin.wat"), code).unwrap();
    d
}

const TOWER: &str = r#"{"ops":[
    {"op":"spawn","name":"Tower","parent":null},
    {"op":"set_property","entity":"$0","path":"height","value":{"Float":12.5}}
]}"#;

fn tower(tag: &str, caps: &str) -> PathBuf {
    plugin_dir(
        tag,
        &format!(
            r#"Plugin(id: "com.example.tower", version: "0.2.0", engine: "^0.1", kind: Wasm,
                provides: [Command("example.tower")], capabilities: [{caps}])"#
        ),
        &forge_wasm::wat::constant(TOWER.as_bytes()),
    )
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

#[test]
fn a_wasm_plugin_loads_reports_and_runs_its_command_once_granted() {
    let dir = tower("tower", "Command(Ordinary)");
    let (ok, out) = forge(&["plugin", "check", &s(&dir)]);
    assert!(ok, "{out}");
    assert!(
        out.contains("plugin com.example.tower 0.2.0 (WASM)"),
        "{out}"
    );
    assert!(out.contains("provides Command(\"example.tower\")"), "{out}");
    assert!(
        out.contains("capability Command(Ordinary): NOT granted"),
        "{out}"
    );

    // Running it without the grant is refused, naming the capability; nothing is applied.
    let (ok, out) = forge(&["plugin", "check", &s(&dir), "--run", "example.tower"]);
    assert!(!ok, "{out}");
    assert!(
        out.contains("CMD-0014") && out.contains("Command(Ordinary)"),
        "{out}"
    );

    // Granted: the command plans in the sandbox and the bus applies it.
    let (ok, out) = forge(&[
        "plugin",
        "check",
        &s(&dir),
        "--grant",
        "Command(Ordinary)",
        "--run",
        "example.tower",
        "{}",
    ]);
    assert!(ok, "{out}");
    assert!(
        out.contains("capability Command(Ordinary): granted"),
        "{out}"
    );
    assert!(out.contains("ran example.tower: 2 change(s)"), "{out}");
    assert!(
        out.contains("entity 0 \"Tower\"") && out.contains("height = Float(12.5)"),
        "{out}"
    );
}

#[test]
fn an_import_the_manifest_does_not_request_is_refused_before_any_code_runs() {
    let reader = forge_wasm::wat::component(
        &["project-read"],
        r#"(func (export "call") (param i32 i32 i32 i32 i32 i32) (result i32)
             (call $read (local.get 4) (local.get 5) (i32.const 64)) (i32.const 64))"#,
    );
    let dir = plugin_dir(
        "peek",
        r#"Plugin(id: "com.example.peek", version: "0.1.0", engine: "^0.1", kind: Wasm)"#,
        &reader,
    );
    let (ok, out) = forge(&["plugin", "check", &s(&dir)]);
    assert!(!ok, "{out}");
    assert!(
        out.contains("WASM-0002") && out.contains("Fs(ProjectRead)"),
        "{out}"
    );
}

#[test]
fn a_bad_grant_is_a_usage_error() {
    let dir = tower("usage", "");
    let (ok, out) = forge(&["plugin", "check", &s(&dir), "--grant", "Everything"]);
    assert!(!ok);
    assert!(out.contains("CLI-0001"), "{out}");
}
