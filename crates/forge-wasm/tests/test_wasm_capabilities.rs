//! M2-13 / E-27: a WASM plugin reaches only what its manifest requests **and** a human has
//! granted in the shared table, checked at each call; the sandbox links nothing else.
//!
//! Positive controls: every denial below is paired with the same call succeeding once the
//! grant exists (so a refusal is the grant check, not a broken fixture), and
//! `positive_control_a_host_that_skips_the_grant_check_is_caught` runs the denial assertion
//! against a stand-in check that ignores the table and requires it to fail.

mod common;

use common::*;
use forge_cmd::{CmdError, Value};
use forge_plugin::{Capability, FsScope, PluginError, SharedGrants};
use forge_wasm::{HostEvent, Limits, WasmError, WasmHost, wat};

const SET_FLAG: &str = r#"{"ops":[{"op":"set_setting","key":"flag","value":{"Bool":true}}]}"#;
const CLEAR_FLAG: &str = r#"{"ops":[{"op":"set_setting","key":"flag","value":null}]}"#;

fn denied(r: Result<forge_cmd::TxnId, forge_cmd::Rejection>) -> String {
    match r {
        Err(rej) => match rej.error {
            CmdError::PolicyRefused { what } => what,
            other => panic!("expected a policy refusal, got {other}"),
        },
        Ok(_) => panic!("the command applied without its grant"),
    }
}

#[test]
fn a_wasm_command_needs_command_ordinary_at_the_moment_it_runs() {
    let grants = SharedGrants::new();
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let m = command_manifest("com.example.flag", "example.flag", "");
    let plugin = host
        .load(m, plan_plugin(SET_FLAG).as_bytes())
        .unwrap_or_else(|e| panic!("{e}"));
    let (mut bus, _) = bus_with(&[], &[&plugin], &grants).unwrap_or_else(|e| panic!("{e}"));

    // No grant: refused before the guest runs, nothing applied, and the denial is recorded.
    let why = denied(run(&mut bus, "example.flag", "{}"));
    assert!(
        why.contains("PLUGIN-0015") && why.contains("Command(Ordinary)"),
        "{why}"
    );
    assert_eq!(bus.project().setting("flag"), None);
    assert!(host.drain_events().iter().any(|e| matches!(
        e,
        HostEvent::Denied { capability, .. } if *capability == ORDINARY
    )));

    // Granted (control): the same call applies — the refusal above was the grant check.
    grants.grant(who("com.example.flag"), ORDINARY);
    let txn = run(&mut bus, "example.flag", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(bus.project().setting("flag"), Some(&Value::Bool(true)));

    // An ordinary plugin command is undoable through the bus like any other (I8).
    forge_cmd::CommandSink::undo(&mut bus, txn).unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(bus.project().setting("flag"), None);

    // Revoked: the very next call is refused (grants are read per call, not at load).
    assert!(grants.revoke(&who("com.example.flag"), ORDINARY));
    denied(run(&mut bus, "example.flag", "{}"));
}

#[test]
fn a_destructive_plan_needs_command_destructive() {
    let grants = SharedGrants::new();
    grants.grant(who("com.example.clear"), ORDINARY);
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let set = host
        .load(
            command_manifest("com.example.set", "example.set", ""),
            plan_plugin(SET_FLAG).as_bytes(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let clear = host
        .load(
            command_manifest("com.example.clear", "example.clear", ""),
            plan_plugin(CLEAR_FLAG).as_bytes(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    grants.grant(who("com.example.set"), ORDINARY);
    let (mut bus, _) = bus_with(&[], &[&set, &clear], &grants).unwrap_or_else(|e| panic!("{e}"));
    run(&mut bus, "example.set", "{}").unwrap_or_else(|r| panic!("{}", r.error));

    // Clearing a setting is destructive (Ch.22.3): Ordinary is not enough.
    let why = denied(run(&mut bus, "example.clear", "{}"));
    assert!(why.contains("Command(Destructive)"), "{why}");
    assert_eq!(bus.project().setting("flag"), Some(&Value::Bool(true)));

    // Control: with the destructive grant the same plan applies.
    grants.grant(who("com.example.clear"), DESTRUCTIVE);
    run(&mut bus, "example.clear", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(bus.project().setting("flag"), None);
}

/// The guest reads `project://<input>` through `project-read` and returns what it got.
fn reader_plugin() -> String {
    wat::component(
        &["project-read"],
        r#"(func (export "call") (param i32 i32 i32 i32 i32 i32) (result i32)
             (call $read (local.get 4) (local.get 5) (i32.const 64))
             (i32.const 64))"#,
    )
}

#[test]
fn an_import_is_linkable_only_if_the_manifest_requests_its_capability() {
    let grants = SharedGrants::new();
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let no_request =
        manifest(r#"Plugin(id: "com.example.peek", version: "0.1.0", engine: "^0.1", kind: Wasm)"#);
    let e = host
        .load(no_request, reader_plugin().as_bytes())
        .map(|_| ())
        .expect_err("an undeclared import must not link");
    assert!(
        matches!(&e, WasmError::UndeclaredImport { capability, .. } if capability == "Fs(ProjectRead)"),
        "{e}"
    );
    assert_eq!(e.code().as_str(), "WASM-0002");

    // Control: requested in the manifest, the same component loads.
    let requested = manifest(
        r#"Plugin(id: "com.example.peek", version: "0.1.0", engine: "^0.1", kind: Wasm,
           capabilities: [Fs(ProjectRead)])"#,
    );
    host.load(requested, reader_plugin().as_bytes())
        .unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn a_linked_import_still_needs_the_grant_on_every_call() {
    let grants = SharedGrants::new();
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    host.set_project_reader(Some(std::sync::Arc::new(|path: &str| {
        if path == "notes.txt" {
            Ok(b"river at the ford".to_vec())
        } else {
            Err(format!("{path}: no such file"))
        }
    })));
    let m = manifest(
        r#"Plugin(id: "com.example.peek", version: "0.1.0", engine: "^0.1", kind: Wasm,
           capabilities: [Fs(ProjectRead)])"#,
    );
    let plugin = host
        .load(m, reader_plugin().as_bytes())
        .unwrap_or_else(|e| panic!("{e}"));
    let item = plugin.item("Probe", "read");

    // Requested but not granted: the guest gets an error naming the capability.
    let e = item.call(b"notes.txt").expect_err("not granted");
    assert!(
        matches!(&e, WasmError::Guest { why, .. } if why.contains("PLUGIN-0015") && why.contains("Fs(ProjectRead)")),
        "{e}"
    );
    let denied_events = host.drain_events();
    assert!(
        denied_events
            .iter()
            .any(|e| matches!(e, HostEvent::Denied { .. }))
    );
    // The denial path really builds the audit string (proves allowed_lazy's what_builder
    // ran, not just that some Denied event exists).
    assert!(
        denied_events
            .iter()
            .any(|e| matches!(e, HostEvent::Denied { what, .. } if what == "read notes.txt")),
        "{denied_events:?}"
    );

    // Granted: the read goes through the host's store reader.
    grants.grant(
        who("com.example.peek"),
        Capability::Fs(FsScope::ProjectRead),
    );
    assert_eq!(
        item.call(b"notes.txt").unwrap_or_else(|e| panic!("{e}")),
        b"river at the ford"
    );
    // An allowed call records zero Denied events, proving allowed_lazy's what_builder
    // never ran on the allowed path (the audit string is built only on denial).
    assert!(
        host.drain_events()
            .iter()
            .all(|e| !matches!(e, HostEvent::Denied { .. }))
    );
    let e = item.call(b"secret.txt").expect_err("missing file");
    assert!(e.to_string().contains("no such file"), "{e}");
}

#[test]
fn the_sandbox_links_nothing_but_forge_plugin_interfaces() {
    let host = WasmHost::new(SharedGrants::new()).unwrap_or_else(|e| panic!("{e}"));
    let m = manifest(
        r#"Plugin(id: "com.example.env", version: "0.1.0", engine: "^0.1", kind: Wasm,
           capabilities: [Fs(ProjectRead), Fs(UserRead), Net(Outbound), Process])"#,
    );
    // Even a manifest requesting everything cannot import WASI: there is nothing to link.
    let wasi = r#"(component
      (import "wasi:cli/environment@0.2.0" (instance
        (export "get-arguments" (func (result (list string))))))
      (core module $m (func (export "call") (param i32 i32 i32 i32 i32 i32) (result i32) (i32.const 0)))
    )"#;
    let e = host
        .load(m, wasi.as_bytes())
        .map(|_| ())
        .expect_err("WASI must not link");
    assert!(
        matches!(&e, WasmError::UnknownImport { import, .. } if import.starts_with("wasi:cli")),
        "{e}"
    );
}

#[test]
fn load_errors_name_the_plugin_and_the_problem() {
    let host = WasmHost::new(SharedGrants::new()).unwrap_or_else(|e| panic!("{e}"));
    let m = || command_manifest("com.example.bad", "example.bad", "");
    let code = |e: WasmError| e.code().as_str();

    let e = host
        .load(m(), b"(component".as_slice())
        .map(|_| ())
        .expect_err("syntax");
    assert_eq!(code(e.clone()), "WASM-0001", "{e}");
    assert!(e.to_string().contains("com.example.bad"));

    let no_export = "(component)";
    let e = host
        .load(m(), no_export.as_bytes())
        .map(|_| ())
        .expect_err("no export");
    assert_eq!(code(e), "WASM-0004");

    let source = manifest(
        r#"Plugin(id: "com.example.native", version: "0.1.0", engine: "^0.1", kind: Source)"#,
    );
    let e = host
        .load(source, wat::constant(b"").as_bytes())
        .map(|_| ())
        .expect_err("source manifest");
    assert_eq!(code(e), "WASM-0009");
}

#[test]
fn a_trap_is_contained_and_the_next_call_starts_fresh() {
    let host = WasmHost::with_limits(
        SharedGrants::new(),
        Limits {
            fuel_per_call: 5_000_000,
            memory_bytes: 1 << 20,
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    // input[0]: 0 = answer "ok", 1 = trap, 2 = spin forever, 3 = allocate 4 MiB.
    let body = format!(
        r#"{}
        (func (export "call") (param i32 i32 i32 i32 i32 i32) (result i32)
          (block $answer
            (br_if $answer (i32.eqz (i32.load8_u (local.get 4))))
            (if (i32.eq (i32.load8_u (local.get 4)) (i32.const 1)) (then unreachable))
            (if (i32.eq (i32.load8_u (local.get 4)) (i32.const 2)) (then (loop $spin (br $spin))))
            (drop (call $alloc (i32.const 4194304))))
          (call $ok (i32.const 1024) (i32.const 2)))"#,
        wat::data(1024, b"ok")
    );
    let m = manifest(
        r#"Plugin(id: "com.example.flaky", version: "0.1.0", engine: "^0.1", kind: Wasm)"#,
    );
    let plugin = host
        .load(m, wat::component(&[], &body).as_bytes())
        .unwrap_or_else(|e| panic!("{e}"));
    let item = plugin.item("Probe", "x");
    assert_eq!(item.call(&[0]).unwrap_or_else(|e| panic!("{e}")), b"ok");
    for (input, what) in [(1u8, "unreachable"), (2, "fuel"), (3, "memory")] {
        let e = item.call(&[input]).expect_err(what);
        assert_eq!(e.code().as_str(), "WASM-0005", "{what}: {e}");
        // Contained: the next call runs on a fresh instance.
        assert_eq!(
            item.call(&[0]).unwrap_or_else(|e| panic!("{e}")),
            b"ok",
            "{what}"
        );
    }
    let traps = host
        .drain_events()
        .into_iter()
        .filter(|e| matches!(e, HostEvent::Trapped { .. }))
        .count();
    assert_eq!(traps, 3);
}

#[test]
fn a_wasm_plugin_conflicts_like_any_plugin_before_its_code_runs() {
    // A source plugin and a WASM plugin both replace one command: a load error naming both,
    // found from the manifests alone (PLUGIN-0006).
    struct Native(forge_plugin::Manifest);
    impl forge_plugin::SourcePlugin for Native {
        fn manifest(&self) -> &forge_plugin::Manifest {
            &self.0
        }
        fn install(&self, cx: &mut forge_plugin::InstallCx) -> Result<(), PluginError> {
            use forge_plugin::points::{Command, CommandItem};
            cx.add::<Command>(
                "demo.base",
                CommandItem::new(|_, _| Ok(())),
                forge_plugin::Order::Last,
            )?;
            cx.replace::<Command>("demo.base", CommandItem::new(|_, _| Ok(())))
        }
    }
    let native = Native(
        forge_plugin::Manifest::source("com.example.native", "0.1.0", "^0.1")
            .unwrap_or_else(|e| panic!("{e}"))
            .provides("Command", "demo.base2")
            .replaces("Command", "demo.base"),
    );
    let grants = SharedGrants::new();
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let wasm = host
        .load(
            manifest(
                r#"Plugin(id: "com.example.wasm", version: "0.1.0", engine: "^0.1", kind: Wasm,
                   replaces: [Command("demo.base")])"#,
            ),
            plan_plugin(SET_FLAG).as_bytes(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let e = bus_with(&[&native], &[&wasm], &grants)
        .map(|_| ())
        .expect_err("conflict");
    let text = e.to_string();
    assert!(
        text.starts_with("PLUGIN-0006")
            && text.contains("com.example.native")
            && text.contains("com.example.wasm"),
        "{text}"
    );
}

#[test]
fn a_point_without_an_adapter_is_a_named_load_error() {
    struct Brush;
    impl forge_plugin::ExtensionPoint for Brush {
        type Item = String;
        const ID: &'static str = "example.brush";
        const NAME: &'static str = "ExampleBrush";
    }
    let grants = SharedGrants::new();
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let m = manifest(
        r#"Plugin(id: "com.example.brushes", version: "0.1.0", engine: "^0.1", kind: Wasm,
           provides: [ExampleBrush("soft")])"#,
    );
    let plugin = host
        .load(m.clone(), wat::constant(b"soft").as_bytes())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut ext = forge_plugin::Extensions::new();
    ext.define::<Brush>().unwrap_or_else(|e| panic!("{e}"));
    let e = forge_plugin::loader::load_hosted(&mut ext, &[], &[&plugin], &[], &grants.snapshot())
        .map(|_| ())
        .expect_err("no adapter");
    let text = e.to_string();
    assert!(
        text.starts_with("PLUGIN-0016")
            && text.contains("WASM-0007")
            && text.contains("ExampleBrush"),
        "{text}"
    );

    // Control: once the host adapts the point, the same plugin installs and serves the item.
    let mut host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    host.adapt::<Brush>(|item| {
        let out = item.call(b"")?;
        String::from_utf8(out).map_err(|e| WasmError::BadOutput {
            plugin: item.plugin().to_string(),
            item: item.name(),
            why: e.to_string(),
        })
    });
    let plugin = host
        .load(m, wat::constant(b"soft").as_bytes())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut ext = forge_plugin::Extensions::new();
    ext.define::<Brush>().unwrap_or_else(|e| panic!("{e}"));
    let report =
        forge_plugin::loader::load_hosted(&mut ext, &[], &[&plugin], &[], &grants.snapshot())
            .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(report.installed, vec![plugin.id().clone()]);
    assert_eq!(
        ext.registry::<Brush>()
            .and_then(|r| r.get("soft"))
            .map(String::as_str),
        Some("soft")
    );
}

#[test]
fn a_wasm_preset_and_a_chained_command_install_through_the_loader() {
    use forge_plugin::points::{Preset, PresetKind};
    let grants = SharedGrants::new();
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let preset = host
        .load(
            manifest(
                r#"Plugin(id: "com.example.rivers", version: "0.1.0", engine: "^0.1", kind: Wasm,
                   provides: [Preset("rivers")])"#,
            ),
            wat::constant(br#"(label: "Rivers", kind: ThreeD, defaults: {"water.level": "0.4"})"#)
                .as_bytes(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let mut ext = forge_plugin::Extensions::new();
    ext.define::<Preset>().unwrap_or_else(|e| panic!("{e}"));
    forge_plugin::loader::load_hosted(&mut ext, &[], &[&preset], &[], &grants.snapshot())
        .unwrap_or_else(|e| panic!("{e}"));
    let p = ext
        .registry::<Preset>()
        .and_then(|r| r.get("rivers"))
        .unwrap_or_else(|| panic!("the preset is registered"));
    assert_eq!((p.label.as_str(), p.kind), ("Rivers", PresetKind::ThreeD));
    assert_eq!(
        p.defaults.get("water.level").map(String::as_str),
        Some("0.4")
    );

    // A WASM chain on a source command: the wrapped plan runs, then the guest's.
    struct Base(forge_plugin::Manifest);
    impl forge_plugin::SourcePlugin for Base {
        fn manifest(&self) -> &forge_plugin::Manifest {
            &self.0
        }
        fn install(&self, cx: &mut forge_plugin::InstallCx) -> Result<(), PluginError> {
            use forge_plugin::points::{Command, CommandItem};
            cx.add::<Command>(
                "demo.base",
                CommandItem::new(|b, _| b.set_setting("base", Some(Value::Int(1)))),
                forge_plugin::Order::Last,
            )
        }
    }
    let base = Base(
        forge_plugin::Manifest::source("com.example.base", "0.1.0", "^0.1")
            .unwrap_or_else(|e| panic!("{e}"))
            .provides("Command", "demo.base"),
    );
    let chain = host
        .load(
            manifest(
                r#"Plugin(id: "com.example.after", version: "0.1.0", engine: "^0.1", kind: Wasm,
                   chains: [Command("demo.base")])"#,
            ),
            plan_plugin(SET_FLAG).as_bytes(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    grants.grant(who("com.example.after"), ORDINARY);
    let (mut bus, ext) = bus_with(&[&base], &[&chain], &grants).unwrap_or_else(|e| panic!("{e}"));
    run(&mut bus, "demo.base", "{}").unwrap_or_else(|r| panic!("{}", r.error));
    assert_eq!(bus.project().setting("base"), Some(&Value::Int(1)));
    assert_eq!(bus.project().setting("flag"), Some(&Value::Bool(true)));
    let prov = ext
        .registry::<forge_plugin::points::Command>()
        .and_then(|r| r.provenance("demo.base"))
        .unwrap_or_else(|| panic!("provenance"));
    assert_eq!(prov.owner.as_str(), "com.example.base");
    assert_eq!(
        prov.chained_by
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>(),
        ["com.example.after"]
    );
}

/// The denial assertion used above, over any grant check: a check that ignores the table
/// must be caught by it.
fn assert_refuses_ungranted(check: &dyn Fn(&SharedGrants) -> bool) -> Result<(), String> {
    let g = SharedGrants::new();
    if check(&g) {
        return Err("an ungranted call was allowed".into());
    }
    g.grant(who("com.example.flag"), ORDINARY);
    if !check(&g) {
        return Err("a granted call was refused".into());
    }
    Ok(())
}

#[test]
fn positive_control_a_host_that_skips_the_grant_check_is_caught() {
    let real = |g: &SharedGrants| g.check(&who("com.example.flag"), ORDINARY).is_ok();
    assert_eq!(assert_refuses_ungranted(&real), Ok(()));
    let skips = |_: &SharedGrants| true;
    assert!(assert_refuses_ungranted(&skips).is_err());

    // The real path: a host plugin item's own check consults the shared table.
    let grants = SharedGrants::new();
    let host = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    let plugin = host
        .load(
            command_manifest("com.example.flag", "example.flag", ""),
            plan_plugin(SET_FLAG).as_bytes(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let item = plugin.item("Command", "example.flag");
    let via_item = |g: &SharedGrants| {
        if !g.same_table(&grants) {
            // Mirror the table under test into the host's table, then ask the item.
            for cap in [ORDINARY] {
                if g.has(&who("com.example.flag"), cap) {
                    grants.grant(who("com.example.flag"), cap);
                } else {
                    grants.revoke(&who("com.example.flag"), cap);
                }
            }
        }
        item.check(ORDINARY).is_ok()
    };
    assert_eq!(assert_refuses_ungranted(&via_item), Ok(()));
}
