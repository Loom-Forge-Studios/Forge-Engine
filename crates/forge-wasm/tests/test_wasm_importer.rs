//! `test_wasm_importer` (WP-21, Ch.32 §32.7, Ch.8): **a WASM plugin is an importer** in a real
//! asset database, through the ordinary loader and `forge_wasm::importer::adapt_importers`.
//!
//! * An echo importer (`fmat`: its source is the artefact) imports a project file: the
//!   sidecar names it, the artefact's blob is the source's bytes, a reimport of unchanged
//!   input does not run it again.
//! * **Hot reload** changes its version (declared version mixed with the code's hash): a
//!   reimport after new code runs it again; the same code is the same version in any run.
//! * **Dependencies** (`needs`): an importer that asks for another file gets it — only with
//!   `Fs(ProjectRead)` granted, checked at that moment against the shared grant table — and
//!   the file is recorded as a dependency of the import.
//! * The protocol's encoders and decoders agree (a Rust-side round trip).
//!
//! Positive control (W2): `positive_control_an_ungranted_dependency_read_fails` — the
//! dependency import with no grant fails naming the capability, and the same import succeeds
//! once a person grants it.

use bytes::Bytes;
use forge_asset::{AssetServer, FirstPartyAssets, Settings};
use forge_plugin::{
    Capability, Extensions, FsScope, Grants, Manifest, Principal, SharedGrants, loader,
};
use forge_store::StorePath;
use forge_wasm::importer::{
    InputHeader, OutArtefact, OutputHeader, adapt_importers, decode_input, decode_output,
    encode_input, encode_output,
};
use forge_wasm::{WasmHost, WasmPlugin, wat};

const DESCRIBE: &str = r#"{"version":1,"extensions":["fmat"]}"#;
const OUT_HEADER: &str = r#"{"artefacts":[{"label":"","kind":"material","name":""}]}"#;

/// The echo importer: describe on empty input; otherwise the source is the main artefact.
/// With `needs_dep`: until a dependency is supplied it answers `needs: ["b.fmat"]`, then the
/// dependency's bytes are the artefact.
fn importer_code(needs_dep: bool, version: u32) -> String {
    let describe = DESCRIBE.replace("\"version\":1", &format!("\"version\":{version}"));
    let oh = OUT_HEADER.len();
    let needs = r#"{"artefacts":[],"needs":["b.fmat"]}"#;
    let mut needs_block = (needs.len() as u32).to_le_bytes().to_vec();
    needs_block.extend_from_slice(needs.as_bytes());
    let payload = if needs_dep {
        // The dependency block follows the source: [u32 len][bytes].
        r#"
      (local.set $end (i32.add (i32.add (i32.const 8) (local.get $hl)) (local.get $sl)))
      (if (i32.eq (local.get $inl) (local.get $end))
        (then (return (call $ok (i32.const 3072) (i32.const NEEDSLEN)))))
      (local.set $sl (i32.load (i32.add (local.get $in) (local.get $end))))
      (local.set $src (i32.add (i32.add (local.get $in) (local.get $end)) (i32.const 4)))"#
            .replace("NEEDSLEN", &needs_block.len().to_string())
    } else {
        String::new()
    };
    wat::component(
        &[],
        &format!(
            r#"{d1}
    {d2}
    {d3}
    (func (export "call") (param $p i32) (param $pn i32) (param $k i32) (param $kn i32)
                          (param $in i32) (param $inl i32) (result i32)
      (local $hl i32) (local $src i32) (local $sl i32) (local $out i32) (local $n i32)
      (local $end i32)
      (if (i32.eqz (local.get $inl))
        (then (return (call $ok (i32.const 1024) (i32.const {dl})))))
      (local.set $hl (i32.load (local.get $in)))
      (local.set $sl (i32.load (i32.add (i32.add (local.get $in) (i32.const 4)) (local.get $hl))))
      (local.set $src (i32.add (i32.add (local.get $in) (i32.const 8)) (local.get $hl)))
      {payload}
      (local.set $n (i32.add (i32.const {fixed}) (local.get $sl)))
      (local.set $out (call $alloc (local.get $n)))
      (i32.store (local.get $out) (i32.const {oh}))
      (memory.copy (i32.add (local.get $out) (i32.const 4)) (i32.const 2048) (i32.const {oh}))
      (i32.store (i32.add (local.get $out) (i32.const {oh4})) (local.get $sl))
      (memory.copy (i32.add (local.get $out) (i32.const {oh8})) (local.get $src) (local.get $sl))
      (call $ok (local.get $out) (local.get $n)))"#,
            d1 = wat::data(1024, describe.as_bytes()),
            d2 = wat::data(2048, OUT_HEADER.as_bytes()),
            d3 = wat::data(3072, &needs_block),
            dl = describe.len(),
            fixed = 8 + oh,
            oh4 = 4 + oh,
            oh8 = 8 + oh,
        ),
    )
}

fn plugin(host: &WasmHost, needs_dep: bool) -> WasmPlugin {
    let m = Manifest::parse(
        r#"Plugin(id: "com.example.fmat", version: "0.1.0", engine: "^0.1", kind: Wasm,
            provides: [Importer("fmat")], capabilities: [Fs(ProjectRead)])"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    host.load(m, importer_code(needs_dep, 1).as_bytes())
        .unwrap_or_else(|e| panic!("{e}"))
}

/// The asset points, the kernel's asset types and `p` loaded through the ordinary loader.
fn extensions(p: &WasmPlugin) -> Extensions {
    let mut x = Extensions::new();
    AssetServer::define_points(&mut x).unwrap_or_else(|e| panic!("{e}"));
    let types = FirstPartyAssets::new().unwrap_or_else(|e| panic!("{e}"));
    loader::load_hosted(&mut x, &[&types], &[p], &[], &Grants::new())
        .unwrap_or_else(|e| panic!("{e}"));
    x
}

fn host(grants: &SharedGrants) -> WasmHost {
    let mut h = WasmHost::new(grants.clone()).unwrap_or_else(|e| panic!("{e}"));
    adapt_importers(&mut h);
    h
}

fn path(p: &str) -> StorePath {
    StorePath::new(p).unwrap_or_else(|e| panic!("{e}"))
}

fn artefact(server: &AssetServer, p: &str) -> Bytes {
    let id = server
        .id_of(&path(p))
        .unwrap_or_else(|| panic!("{p} is not imported"));
    let blob = server
        .info(id)
        .and_then(|i| i.blob)
        .unwrap_or_else(|| panic!("{p} has no artefact"));
    server
        .vfs()
        .blob_get(blob)
        .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn a_wasm_importer_imports_and_reimports_by_its_codes_version() {
    let grants = SharedGrants::new();
    let h = host(&grants);
    let p = plugin(&h, false);
    let x = extensions(&p);
    let src = b"(colour: (0.5, 0.25, 1.0))".to_vec();
    let (mut server, _) =
        AssetServer::open_memory("test", &[("mats/a.fmat".into(), src.clone())], &x)
            .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(server.importer_for(&path("mats/a.fmat")), Some("fmat"));
    let mut ev = Vec::new();
    server
        .import(&path("mats/a.fmat"), Settings::new(), &mut ev)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(artefact(&server, "mats/a.fmat").as_ref(), &src[..]);
    let sc = server
        .sidecar(&path("mats/a.fmat"))
        .and_then(|s| s.import.clone())
        .unwrap_or_else(|| panic!("an import record"));
    assert_eq!(sc.importer, "fmat");
    let v1 = sc.importer_version;
    // Unchanged input: the importer does not run again.
    let runs = server.import_runs();
    server
        .reimport(&path("mats/a.fmat"), &mut ev)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(server.import_runs(), runs, "a fresh import ran again");
    // The same code is the same version in another host (another run).
    let version = |x: &Extensions| {
        x.registry::<forge_asset::ImporterPoint>()
            .and_then(|r| r.get("fmat"))
            .map(|i| i.version())
    };
    let again = plugin(&host(&SharedGrants::new()), false);
    assert_eq!(version(&extensions(&again)), Some(v1));
    assert_eq!(version(&x), Some(v1));
    // Hot reload: new code (a new declared version too, here) is a new version, and a
    // reimport runs it.
    p.reload(importer_code(false, 2).as_bytes())
        .unwrap_or_else(|e| panic!("{e}"));
    server
        .reimport(&path("mats/a.fmat"), &mut ev)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(server.import_runs(), runs + 1, "new code did not reimport");
    let v2 = server
        .sidecar(&path("mats/a.fmat"))
        .and_then(|s| s.import.clone())
        .map(|r| r.importer_version)
        .unwrap_or_default();
    assert_ne!(v1, v2, "the version did not follow the code");
    assert_ne!(v2, 0);
}

fn dependency_import(grant: bool) -> Result<(), String> {
    let grants = SharedGrants::new();
    let h = host(&grants);
    let p = plugin(&h, true);
    let x = extensions(&p);
    if grant {
        grants.grant(
            Principal::Plugin(p.id().clone()),
            Capability::Fs(FsScope::ProjectRead),
        );
    }
    let files = vec![
        ("mats/a.fmat".to_string(), b"stub".to_vec()),
        ("mats/b.fmat".to_string(), b"the real material".to_vec()),
    ];
    let (mut server, _) =
        AssetServer::open_memory("test", &files, &x).map_err(|e| e.to_string())?;
    let mut ev = Vec::new();
    server
        .import(&path("mats/a.fmat"), Settings::new(), &mut ev)
        .map_err(|e| e.to_string())?;
    if artefact(&server, "mats/a.fmat").as_ref() != b"the real material" {
        return Err("the dependency's bytes are not the artefact".into());
    }
    let deps = server
        .sidecar(&path("mats/a.fmat"))
        .and_then(|s| s.import.clone())
        .map(|r| r.deps)
        .unwrap_or_default();
    if !deps.iter().any(|(k, _)| k == "mats/b.fmat") {
        return Err(format!("the dependency is not recorded: {deps:?}"));
    }
    Ok(())
}

#[test]
fn a_wasm_importer_reads_a_dependency_when_granted() {
    dependency_import(true).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_ungranted_dependency_read_fails() {
    let e = dependency_import(false).expect_err("an ungranted dependency read must fail");
    assert!(e.contains("Fs(ProjectRead)"), "{e}");
}

#[test]
fn the_protocol_round_trips() {
    let h = InputHeader {
        path: "a/b.x".into(),
        settings: [("k".to_string(), "v".to_string())].into_iter().collect(),
        deps: vec!["c.bin".into()],
    };
    let bytes = encode_input(&h, b"source", &[b"dep"]).unwrap_or_else(|e| panic!("{e}"));
    let (h2, s, d) = decode_input(&bytes).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((h2, s, d), (h, b"source".to_vec(), vec![b"dep".to_vec()]));
    let o = OutputHeader {
        artefacts: vec![OutArtefact {
            label: String::new(),
            kind: "mesh".into(),
            name: "A".into(),
        }],
        needs: Vec::new(),
    };
    let bytes = encode_output(&o, &[b"mesh bytes"]).unwrap_or_else(|e| panic!("{e}"));
    let (o2, a) = decode_output(&bytes).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(o2, o);
    assert_eq!(a[0].as_ref(), b"mesh bytes");
    // Truncated or trailing bytes are errors, never a silent partial import.
    assert!(decode_output(&bytes[..bytes.len() - 1]).is_err());
    let mut long = bytes.clone();
    long.push(0);
    assert!(decode_output(&long).is_err());
}
