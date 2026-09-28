//! The loader (Ch.32.4): conflicts are load errors naming both plugins, found from the
//! manifests before any plugin code runs; every operation is declared; the result does not
//! depend on the order plugins were discovered in.

use std::cell::Cell;
use std::collections::BTreeMap;

use forge_plugin::loader::{load, load_against};
use forge_plugin::points::{Preset, PresetDescriptor, PresetKind};
use forge_plugin::{
    Capability, Extensions, FsScope, GpuUse, Grants, InstallCx, Manifest, Order, PluginError,
    PluginId, Principal, SourcePlugin,
};

fn preset(label: &str) -> PresetDescriptor {
    PresetDescriptor {
        label: label.into(),
        kind: PresetKind::ThreeD,
        defaults: BTreeMap::new(),
        files: BTreeMap::new(),
    }
}

/// A source plugin whose install is a closure over the context.
struct P {
    m: Manifest,
    f: fn(&mut InstallCx) -> Result<(), PluginError>,
    ran: Cell<bool>,
}

impl P {
    fn new(m: Manifest, f: fn(&mut InstallCx) -> Result<(), PluginError>) -> Self {
        Self {
            m,
            f,
            ran: Cell::new(false),
        }
    }
}

impl SourcePlugin for P {
    fn manifest(&self) -> &Manifest {
        &self.m
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        self.ran.set(true);
        (self.f)(cx)
    }
}

fn host() -> Extensions {
    let mut x = Extensions::new();
    x.define::<Preset>().expect("define");
    x
}

fn src(id: &str) -> Manifest {
    Manifest::source(id, "1.0.0", "^0.1").expect("manifest")
}

fn core() -> P {
    P::new(
        src("forge.presets")
            .provides("Preset", "forge.3d")
            .provides("Preset", "forge.2d"),
        |cx| {
            cx.add::<Preset>("forge.3d", preset("3D"), Order::Last)?;
            cx.add::<Preset>("forge.2d", preset("2D"), Order::First)
        },
    )
}

fn labels(x: &Extensions) -> Vec<String> {
    x.registry::<Preset>()
        .expect("defined")
        .iter()
        .map(|(_, p)| p.label.clone())
        .collect()
}

#[test]
fn replace_remove_and_chain_apply_whatever_the_discovery_order() {
    let replacer = P::new(
        src("com.acme.better3d").replaces("Preset", "forge.3d"),
        |cx| cx.replace::<Preset>("forge.3d", preset("Better 3D")),
    );
    let chainer = P::new(src("com.acme.tagger").chains("Preset", "forge.3d"), |cx| {
        cx.chain::<Preset>("forge.3d", |mut p| {
            p.label = format!("{} (tagged)", p.label);
            p
        })
    });
    let remover = P::new(src("com.acme.no2d").removes("Preset", "forge.2d"), |cx| {
        cx.remove::<Preset>("forge.2d")
    });
    let base = core();
    // The chainer and replacer sort before `forge.presets`, which provides the target: the
    // phase order still applies every add first, every replace before any chain.
    for order in [
        vec![&base as &dyn SourcePlugin, &replacer, &chainer, &remover],
        vec![&remover, &chainer, &replacer, &base],
    ] {
        let mut x = host();
        let report = load(&mut x, &order, &[], &Grants::new()).expect("loads");
        assert_eq!(labels(&x), ["Better 3D (tagged)"]);
        assert_eq!(report.installed.len(), 4);
        let prov = x
            .registry::<Preset>()
            .and_then(|r| r.provenance("forge.3d"))
            .cloned()
            .expect("provenance");
        assert_eq!(prov.owner.as_str(), "forge.presets");
        assert_eq!(
            prov.replaced_by.map(|p| p.to_string()),
            Some("com.acme.better3d".into())
        );
        assert_eq!(prov.chained_by.len(), 1);
    }
}

#[test]
fn conflicting_replaces_are_a_load_error_naming_both_before_any_code_runs() {
    let a = P::new(src("com.alpha.x").replaces("Preset", "forge.3d"), |cx| {
        cx.replace::<Preset>("forge.3d", preset("alpha"))
    });
    let b = P::new(src("com.beta.y").replaces("Preset", "forge.3d"), |cx| {
        cx.replace::<Preset>("forge.3d", preset("beta"))
    });
    let base = core();
    let mut x = host();
    let e = load(&mut x, &[&base, &b, &a], &[], &Grants::new()).expect_err("conflict");
    assert_eq!(e.code().as_str(), "PLUGIN-0006");
    let msg = e.to_string();
    assert!(
        msg.contains("com.alpha.x") && msg.contains("com.beta.y"),
        "{msg}"
    );
    assert!(msg.contains("Preset(\"forge.3d\")"), "{msg}");
    assert!(
        !a.ran.get() && !b.ran.get() && !base.ran.get(),
        "no plugin code ran"
    );
    assert!(labels(&x).is_empty(), "nothing was applied");

    // A WASM plugin's manifest takes part in the check even though it cannot install yet.
    let wasm = Manifest::parse(
        r#"Plugin(id: "com.gamma.z", version: "1.0.0", engine: "^0.1", kind: Wasm,
                  removes: [ Preset("forge.3d") ])"#,
    )
    .expect("parses");
    let mut x = host();
    let e = load(&mut x, &[&base, &a], &[wasm], &Grants::new()).expect_err("conflict");
    let msg = e.to_string();
    assert!(
        msg.contains("com.alpha.x") && msg.contains("com.gamma.z"),
        "{msg}"
    );

    // Positive control: either plugin alone loads.
    let mut x = host();
    load(&mut x, &[&base, &a], &[], &Grants::new()).expect("alone it loads");
    assert_eq!(labels(&x), ["2D", "alpha"]);
}

#[test]
fn every_other_manifest_level_error_is_reported() {
    let base = core();
    let dup = P::new(src("com.other.dup").provides("Preset", "forge.2d"), |cx| {
        cx.add::<Preset>("forge.2d", preset("again"), Order::Last)
    });
    let code = |r: Result<forge_plugin::LoadReport, PluginError>| {
        r.map(|_| ())
            .map_err(|e| (e.code().as_str(), e.to_string()))
    };
    let (c, msg) = code(load(&mut host(), &[&base, &dup], &[], &Grants::new())).expect_err("dup");
    assert_eq!(c, "PLUGIN-0003");
    assert!(
        msg.contains("forge.presets") && msg.contains("com.other.dup"),
        "{msg}"
    );

    let old = P::new(
        Manifest::source("com.old.p", "1.0.0", "^0.0.5").expect("m"),
        |_| Ok(()),
    );
    assert_eq!(
        code(load(&mut host(), &[&old], &[], &Grants::new())).map_err(|e| e.0),
        Err("PLUGIN-0008")
    );
    let unknown = P::new(src("com.x.p").provides("RenderPass", "bloom"), |_| Ok(()));
    assert_eq!(
        code(load(&mut host(), &[&unknown], &[], &Grants::new())).map_err(|e| e.0),
        Err("PLUGIN-0010")
    );
    let twin = core();
    assert_eq!(
        code(load(&mut host(), &[&base, &twin], &[], &Grants::new())).map_err(|e| e.0),
        Err("PLUGIN-0009")
    );
    let sneaky = P::new(src("com.sneaky.p"), |cx| {
        cx.replace::<Preset>("forge.3d", preset("sneaky"))
    });
    let (c, msg) =
        code(load(&mut host(), &[&base, &sneaky], &[], &Grants::new())).expect_err("undeclared");
    assert_eq!(c, "PLUGIN-0011");
    assert!(
        msg.contains("com.sneaky.p") && msg.contains("replace"),
        "{msg}"
    );
    let chains_removed = P::new(src("com.chain.p").chains("Preset", "forge.2d"), |cx| {
        cx.chain::<Preset>("forge.2d", |p| p)
    });
    let remover = P::new(src("com.remove.p").removes("Preset", "forge.2d"), |cx| {
        cx.remove::<Preset>("forge.2d")
    });
    assert_eq!(
        code(load(
            &mut host(),
            &[&base, &chains_removed, &remover],
            &[],
            &Grants::new()
        ))
        .map_err(|e| e.0),
        Err("PLUGIN-0006")
    );
    let boom = P::new(src("com.boom.p"), |_| panic!("install exploded"));
    let (c, msg) = code(load(&mut host(), &[&boom], &[], &Grants::new())).expect_err("panic");
    assert_eq!(c, "PLUGIN-0013");
    assert!(msg.contains("install exploded"));
    let wasm_as_source = P::new(
        Manifest::parse(r#"Plugin(id: "com.w.p", version: "1.0.0", engine: "^0.1", kind: Wasm)"#)
            .expect("m"),
        |_| Ok(()),
    );
    assert_eq!(
        code(load(&mut host(), &[&wasm_as_source], &[], &Grants::new())).map_err(|e| e.0),
        Err("PLUGIN-0007")
    );
}

#[test]
fn two_plugins_removing_the_same_item_agree() {
    let base = core();
    let r1 = P::new(src("com.one.r").removes("Preset", "forge.2d"), |cx| {
        cx.remove::<Preset>("forge.2d")
    });
    let r2 = P::new(src("com.two.r").removes("Preset", "forge.2d"), |cx| {
        cx.remove::<Preset>("forge.2d")
    });
    let mut x = host();
    load(&mut x, &[&base, &r1, &r2], &[], &Grants::new()).expect("both removals agree");
    assert_eq!(labels(&x), ["3D"]);
}

#[test]
fn wasm_plugins_are_reported_not_installed_and_capabilities_requested_vs_granted() {
    let rivers = Manifest::parse(
        r#"Plugin(
          id: "com.example.rivers", version: "0.3.1", engine: "^0.1", kind: Wasm,
          provides: [ Preset("rivers") ],
          capabilities: [ Fs(ProjectRead), Gpu(Compute) ],
        )"#,
    )
    .expect("parses");
    let mut grants = Grants::new();
    let who = Principal::Plugin(PluginId::new("com.example.rivers").expect("id"));
    grants.grant(who, Capability::Fs(FsScope::ProjectRead));
    let base = core();
    let mut x = host();
    let report = load(&mut x, &[&base], &[rivers], &grants).expect("loads");
    assert_eq!(report.not_installed.len(), 1);
    assert_eq!(report.not_installed[0].1.code().as_str(), "PLUGIN-0014");
    let caps = report
        .capabilities
        .iter()
        .find(|c| c.plugin.as_str() == "com.example.rivers")
        .expect("reported");
    assert_eq!(
        caps.requested,
        [
            Capability::Fs(FsScope::ProjectRead),
            Capability::Gpu(GpuUse::Compute)
        ]
    );
    assert_eq!(
        caps.granted,
        [Capability::Fs(FsScope::ProjectRead)],
        "only what a human granted"
    );
    assert!(!labels(&x).contains(&"rivers".to_string()));
}

#[test]
fn the_engine_requirement_is_checked_against_the_kernel() {
    let p = P::new(
        Manifest::source("com.v.p", "1.0.0", ">=0.2").expect("m"),
        |_| Ok(()),
    );
    let v01 = semver::Version::new(0, 1, 0);
    let v02 = semver::Version::new(0, 2, 3);
    assert!(load_against(&mut host(), &[&p], &[], &Grants::new(), &v01).is_err());
    assert!(load_against(&mut host(), &[&p], &[], &Grants::new(), &v02).is_ok());
}
