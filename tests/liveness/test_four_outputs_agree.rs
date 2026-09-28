//! Ch.6 guard — `test_four_outputs_agree`: for every `#[forge_api]` item, the `bevy_reflect`
//! registration, the blueprint node descriptor, the JSON Schema fragment and (if it mutates)
//! the command variant describe the same arity, names, types, units, docs and purity.
//! "A drift here is silent and catastrophic."
//!
//! The items: [`engine_registry`] registers every `#[forge_api]` item in the engine. No crate
//! above forge-reflect exists yet (forge-cmd is WP-05), so today that is the representative
//! set below — a mutating fn (the Ch.6 `apply_impulse` example), a pure fn, a reading fn, a
//! struct carrying every inspector annotation, a unit enum and a data enum. Each crate that
//! lands `#[forge_api]` items adds its registration call there.
//!
//! This file lives in `forge-tests`, which does **not** depend on `bevy_reflect`: it also
//! proves `#[forge_api]` works in a crate that only depends on `forge-reflect`.
//!
//! **Positive controls (W2):**
//! * `positive_control_drifted_descriptors_fail` — every kind of drift (a dropped pin, a
//!   changed type, unit, tooltip, range, purity, command, variant, reflect type) is applied to
//!   real registered items; the checker and the registry must refuse every one.
//! * `positive_control_mutate_drift_build_fails` — rebuilds this file with the macro emitting
//!   drifted node descriptors (`--features mutate-four-outputs`); the guard must FAIL there.

// A test harness: helpers panic on a broken fixture by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use forge_core::bevy_ecs;
use forge_core::bevy_ecs::component::Component;
use forge_core::{EntityId, Error, World};
use forge_frames::{DVec3, FrameVel};
use forge_reflect::{
    ApiItem, ForgeRegistry, ItemKind, PinKind, Purity, ReflectDesc, ReflectError, check_agreement,
    connect, forge_api,
};
use serde_json::json;

const DRIFT_MARKER: &str = "FOUR OUTPUTS DRIFT";

// ---- the representative items ------------------------------------------------------------

/// Momentum of a body (the state `apply_impulse` changes).
#[derive(Component)]
pub struct Momentum(pub FrameVel);

#[forge_api(category = "Physics", mutates)]
/// Applies an impulse to a body, in the body's own frame.
pub fn apply_impulse(
    world: &mut World,
    #[forge(entity, doc = "The body to push.")] target: EntityId,
    #[forge(units = "N·s", doc = "The impulse, in the body's frame.")] impulse: FrameVel,
) -> Result<(), Error> {
    let mut m = world.get_mut::<Momentum>(target)?;
    let p = m.0.local;
    m.0.local = DVec3 {
        x: p.x + impulse.local.x,
        y: p.y + impulse.local.y,
        z: p.z + impulse.local.z,
    };
    Ok(())
}

/// A body's momentum.
#[forge_api(category = "Physics", reads, returns(units = "N·s", name = "Momentum"))]
pub fn momentum_of(
    world: &World,
    #[forge(entity, doc = "The body.")] target: EntityId,
) -> Result<FrameVel, Error> {
    Ok(world.get::<Momentum>(target)?.0)
}

/// Kinetic energy of a moving mass.
#[forge_api(category = "Physics", returns(units = "J"))]
pub fn kinetic_energy(
    #[forge(units = "kg", min = 0.0, doc = "The mass.")] mass: f64,
    #[forge(units = "m/s", doc = "The speed.")] speed: f64,
) -> f64 {
    0.5 * mass * speed * speed
}

/// How a thruster throttles.
#[forge_api]
#[derive(Clone, Debug, PartialEq)]
pub enum ThrustMode {
    /// Full thrust or nothing.
    OnOff,
    /// Any level between zero and full.
    #[forge(name = "Throttled (continuous)")]
    Throttled,
}

/// A collision shape.
#[forge_api(category = "Physics")]
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// A ball.
    Sphere {
        /// Radius of the ball.
        #[forge(units = "m", min = 0.0)]
        radius: f64,
    },
    /// An axis-aligned box.
    Cuboid {
        /// Half extent along x.
        #[forge(units = "m", min = 0.0)]
        half_x: f64,
        /// Half extent along y.
        #[forge(units = "m", min = 0.0)]
        half_y: f64,
    },
    /// Collides with nothing.
    Empty,
}

/// Tuning for one thruster.
#[forge_api(category = "Propulsion")]
#[derive(Clone, Debug, PartialEq)]
pub struct ThrusterConfig {
    /// Thrust at full throttle.
    #[forge(units = "kN", range = 0.0..=500.0, step = 0.5, category = "Output")]
    pub max_thrust: f64,
    /// Fuel burned per second at full thrust.
    #[forge(units = "kg/s", min = 0, name = "Fuel Flow")]
    pub fuel_flow: f64,
    /// How far the nozzle swivels.
    #[forge(units = "deg", range = 0..=30, widget = "angle", category = "Output")]
    pub gimbal: f64,
    /// Is the thruster lit?
    pub enabled: bool,
    /// How it throttles.
    pub mode: ThrustMode,
    /// Serial number stamped at the factory.
    #[forge(read_only)]
    pub serial: u32,
    /// Editor bookkeeping.
    #[forge(hidden)]
    pub revision: u64,
    /// The mount it is attached to.
    #[forge(entity)]
    pub mount: EntityId,
}

/// Every `#[forge_api]` item in the engine. Registration itself refuses a drifted item, so a
/// failure here names the drift.
fn engine_registry() -> ForgeRegistry {
    let mut r = ForgeRegistry::new();
    let results = [
        r.register::<apply_impulse>().map(|_| ()),
        r.register::<momentum_of>().map(|_| ()),
        r.register::<kinetic_energy>().map(|_| ()),
        r.register::<ThrustMode>().map(|_| ()),
        r.register::<Shape>().map(|_| ()),
        r.register::<ThrusterConfig>().map(|_| ()),
    ];
    for res in results {
        if let Err(e) = res {
            panic!("{DRIFT_MARKER}: {e}");
        }
    }
    r
}

const ITEM_COUNT: usize = 6;

// ---- the guard ----------------------------------------------------------------------------

#[test]
fn four_outputs_agree_for_every_item() {
    let reg = engine_registry();
    assert_eq!(reg.items().len(), ITEM_COUNT);
    for item in reg.items() {
        if let Err(e) = check_agreement(item) {
            panic!("{DRIFT_MARKER}: {e}");
        }
        // The things the checker compares must actually be present (a vacuous item that
        // agrees because every output is empty would not be a test).
        match item.kind {
            ItemKind::Fn => {
                let ReflectDesc::Fn { args, .. } = &item.reflect else {
                    panic!("fn with a type reflect desc");
                };
                assert!(!args.is_empty(), "{}: no pins", item.path);
            }
            ItemKind::Struct => assert!(!item.node.inputs.is_empty()),
            ItemKind::Enum => assert!(!item.node.variants.is_empty()),
        }
    }
}

#[test]
fn the_ch6_example_has_all_four_outputs() {
    let reg = engine_registry();
    let item = reg
        .items()
        .find(|i| i.ident == "apply_impulse")
        .expect("registered");
    assert!(item.path.ends_with("::apply_impulse"));
    // 1. reflect: the signature's by-value types.
    let ReflectDesc::Fn { args, ret } = &item.reflect else {
        panic!("fn")
    };
    let types: Vec<&str> = args.iter().map(|a| a.info.type_path()).collect();
    assert_eq!(
        types,
        ["forge_core::entity::EntityId", "forge_frames::FrameVel"]
    );
    assert!(ret.is_none());
    assert!(
        reg.type_registry()
            .get_with_type_path("forge_frames::FrameVel")
            .is_some()
    );
    // 2. node.
    assert_eq!(item.node.purity, Purity::Mutate);
    assert_eq!(
        item.node.doc,
        "Applies an impulse to a body, in the body's own frame."
    );
    assert_eq!(item.node.inputs[0].kind, PinKind::Entity);
    assert!(item.node.inputs[0].meta.entity);
    assert_eq!(
        item.node.inputs[1].unit.as_ref().map(|u| u.text()),
        Some("N·s")
    );
    assert_eq!(item.node.context.len(), 1);
    assert!(item.node.context[0].mutable);
    assert!(item.node.fallible);
    // 3. schema.
    let s = &item.schema;
    assert_eq!(s["required"], json!(["target", "impulse"]));
    assert_eq!(s["properties"]["impulse"]["x-forge-unit"], "N·s");
    assert_eq!(s["properties"]["impulse"]["x-forge-dimension"], "m·kg·s^-1");
    assert_eq!(s["description"], item.node.doc);
    assert_eq!(s["x-forge-purity"], "mutate");
    // 4. command.
    let cmd = item.command.as_ref().expect("a mutating fn is a command");
    assert_eq!(cmd.variant, "ApplyImpulse");
    assert_eq!(cmd.fields.len(), 2);
    assert_eq!(s["x-forge-command"], "ApplyImpulse");

    // The fn itself still works.
    let mut w = World::new();
    let e = w.spawn(Momentum(FrameVel::at_rest(forge_frames::FrameId(0))));
    apply_impulse(
        &mut w,
        e,
        FrameVel::new(
            forge_frames::FrameId(0),
            DVec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
        ),
    )
    .expect("applies");
    assert_eq!(momentum_of(&w, e).expect("reads").local.x, 1.0);
}

#[test]
fn units_are_enforced_on_connection() {
    let reg = engine_registry();
    let get = |id: &str| reg.items().find(|i| i.ident == id).expect(id).clone();
    let (ke, mo, ai) = (
        get("kinetic_energy"),
        get("momentum_of"),
        get("apply_impulse"),
    );
    // momentum_of -> apply_impulse: both FrameVel in N·s.
    connect(&mo.node.outputs[0], &ai.node.inputs[1]).expect("N·s into N·s");
    // kinetic_energy (J, f64) -> speed (m/s, f64): same type, different dimension.
    let e = connect(&ke.node.outputs[0], &ke.node.inputs[1]).expect_err("J into m/s");
    assert!(matches!(e, ReflectError::IncompatibleUnits { .. }), "{e}");
    let err: Error = e.into();
    assert_eq!(err.code().as_str(), "REFLECT-0008");
    // A pin in the same dimension converts (kg into a mass pin in kg: scale 1).
    assert_eq!(
        connect(&ke.node.inputs[0], &ke.node.inputs[0])
            .expect("same")
            .scale,
        1.0
    );
}

#[test]
fn schema_document_serves_the_whole_registry() {
    let doc = engine_registry().schema_document();
    assert_eq!(doc["$id"], "schema://forge");
    let fns = doc["x-forge-functions"].as_object().expect("functions");
    assert_eq!(fns.len(), 3);
    let defs = doc["$defs"].as_object().expect("defs");
    for t in ["ThrusterConfig", "ThrustMode", "Shape"] {
        assert!(
            defs.keys().any(|k| k.ends_with(&format!("::{t}"))),
            "{t} in $defs"
        );
        assert!(
            doc["x-forge-types"]
                .as_object()
                .expect("types")
                .keys()
                .any(|k| k.ends_with(t))
        );
    }
    assert!(defs.contains_key("forge_frames::FrameVel"));
    // A struct field of an enum type references the enum's definition.
    let tc = defs
        .iter()
        .find(|(k, _)| k.ends_with("::ThrusterConfig"))
        .expect("tc")
        .1;
    let mode_ref = tc["properties"]["mode"]["$ref"].as_str().expect("$ref");
    assert!(mode_ref.starts_with("#/$defs/") && mode_ref.ends_with("::ThrustMode"));
    // Inspector metadata reaches the schema.
    let mt = &tc["properties"]["max_thrust"];
    assert_eq!(
        (mt["minimum"].as_f64(), mt["maximum"].as_f64()),
        (Some(0.0), Some(500.0))
    );
    assert_eq!(mt["x-forge-step"], 0.5);
    assert_eq!(tc["properties"]["serial"]["readOnly"], true);
    assert_eq!(tc["properties"]["revision"]["x-forge-hidden"], true);
    assert_eq!(tc["properties"]["fuel_flow"]["title"], "Fuel Flow");
    assert_eq!(
        tc["properties"]["enabled"]["description"],
        "Is the thruster lit?"
    );
    // A data enum is externally tagged.
    let shape = defs
        .iter()
        .find(|(k, _)| k.ends_with("::Shape"))
        .expect("shape")
        .1;
    assert_eq!(shape["oneOf"][2]["const"], "Empty");
    assert_eq!(shape["oneOf"][0]["required"], json!(["Sphere"]));
}

#[test]
fn inspector_is_generated_from_the_annotations() {
    let reg = engine_registry();
    let path = reg
        .items()
        .find(|i| i.ident == "ThrusterConfig")
        .expect("tc")
        .path
        .clone();
    let insp = reg.inspector(&path).expect("inspector");
    assert_eq!(insp.title, "Thruster Config");
    let groups: Vec<(Option<&str>, Vec<&str>)> = insp
        .groups
        .iter()
        .map(|(c, f)| (*c, f.iter().map(|f| f.name).collect()))
        .collect();
    assert_eq!(
        groups,
        [
            (
                None,
                vec!["fuel_flow", "enabled", "mode", "serial", "mount"]
            ),
            (Some("Output"), vec!["max_thrust", "gimbal"]),
        ],
        "hidden `revision` omitted; categories grouped in first-seen order"
    );
    let field = |n: &str| {
        insp.groups
            .iter()
            .flat_map(|(_, f)| f)
            .find(|f| f.name == n)
            .expect(n)
            .clone()
    };
    assert_eq!(field("gimbal").widget, Some("angle"));
    assert_eq!(field("gimbal").units, Some("deg"));
    assert_eq!(field("max_thrust").label, "Max Thrust");
    assert_eq!(field("max_thrust").tooltip, "Thrust at full throttle.");
    assert!(field("serial").read_only);
    assert!(field("mount").entity);
    assert_eq!(field("mode").variants, ["OnOff", "Throttled"]);
    assert_eq!(field("mode").kind, PinKind::Enum);
}

// ---- positive controls ------------------------------------------------------------------

type Mutation = (&'static str, fn(&mut ApiItem) -> bool);

/// Each returns whether it applied to this item.
fn mutations() -> Vec<Mutation> {
    vec![
        ("drop the last node input", |i| {
            i.node.inputs.pop().is_some()
        }),
        ("change a node pin's type", |i| {
            i.node.inputs.first_mut().is_some_and(|p| {
                p.type_path = if p.type_path == "f64" { "i64" } else { "f64" };
                true
            })
        }),
        ("change a node pin's units", |i| {
            i.node
                .inputs
                .iter_mut()
                .find(|p| p.meta.units.is_some())
                .is_some_and(|p| {
                    p.meta.units = Some("m/s²");
                    true
                })
        }),
        ("change a node pin's tooltip", |i| {
            i.node.inputs.first_mut().is_some_and(|p| {
                p.meta.doc = "Something else.";
                true
            })
        }),
        ("widen a node pin's range", |i| {
            i.node
                .inputs
                .iter_mut()
                .find(|p| p.meta.min.is_some())
                .is_some_and(|p| {
                    p.meta.min = Some(-1.0);
                    true
                })
        }),
        ("change the node doc", |i| {
            i.node.doc = "Drifted.";
            true
        }),
        ("rename a schema property", |i| {
            let props = i
                .schema
                .get_mut("properties")
                .and_then(|p| p.as_object_mut());
            props.is_some_and(|p| match p.keys().next().cloned() {
                Some(k) => {
                    let v = p.remove(&k);
                    p.insert(format!("{k}_renamed"), v.unwrap_or_default());
                    true
                }
                None => false,
            })
        }),
        ("change a schema property's type", |i| {
            let props = i
                .schema
                .get_mut("properties")
                .and_then(|p| p.as_object_mut());
            props.is_some_and(|p| match p.values_mut().next() {
                Some(v) => {
                    v["x-forge-type"] = json!("u8");
                    true
                }
                None => false,
            })
        }),
        ("drop a schema unit", |i| {
            let props = i
                .schema
                .get_mut("properties")
                .and_then(|p| p.as_object_mut());
            props.is_some_and(|p| {
                p.values_mut()
                    .find_map(|v| v.as_object_mut().and_then(|o| o.remove("x-forge-unit")))
                    .is_some()
            })
        }),
        ("flip purity", |i| {
            if i.kind != ItemKind::Fn {
                return false;
            }
            i.node.purity = match i.node.purity {
                Purity::Pure => Purity::Mutate,
                Purity::Read | Purity::Mutate => Purity::Pure,
            };
            true
        }),
        ("drop the command of a mutating fn", |i| {
            i.command.take().is_some()
        }),
        ("drop a command field", |i| {
            i.command.as_mut().is_some_and(|c| c.fields.pop().is_some())
        }),
        (
            "change the reflected type of an argument",
            |i| match &mut i.reflect {
                ReflectDesc::Fn { args, .. } => args.first_mut().is_some_and(|a| {
                    a.info = <u8 as forge_reflect::bevy_reflect::Typed>::type_info();
                    true
                }),
                ReflectDesc::Type(_) => false,
            },
        ),
        ("drop the fn's output pin", |i| {
            i.kind == ItemKind::Fn && i.node.outputs.pop().is_some()
        }),
        ("drop an enum variant from the node", |i| {
            i.node.variants.pop().is_some()
        }),
        ("change a variant field's units", |i| {
            i.node
                .variants
                .iter_mut()
                .flat_map(|v| &mut v.fields)
                .find(|p| p.meta.units.is_some())
                .is_some_and(|p| {
                    p.meta.units = Some("kg");
                    true
                })
        }),
    ]
}

#[test]
fn positive_control_drifted_descriptors_fail() {
    let reg = engine_registry();
    let mut applied = vec![0usize; mutations().len()];
    for item in reg.items() {
        for (k, (what, mutate)) in mutations().into_iter().enumerate() {
            let mut drifted = item.clone();
            if !mutate(&mut drifted) {
                continue;
            }
            applied[k] += 1;
            let err = check_agreement(&drifted)
                .expect_err(&format!("{}: `{what}` was not detected", item.path));
            assert!(matches!(err, ReflectError::Drift { .. }), "{what}: {err}");
            // And the registry refuses to serve it.
            let mut fresh = ForgeRegistry::new();
            assert!(
                fresh.insert(drifted).is_err(),
                "{}: registry accepted `{what}`",
                item.path
            );
        }
    }
    for ((what, _), n) in mutations().iter().zip(&applied) {
        assert!(
            *n > 0,
            "mutation `{what}` never applied: the control is vacuous for it"
        );
    }
}

#[test]
fn positive_control_mutate_drift_build_fails() {
    if forge_reflect::MUTATE_DRIFT {
        return; // we ARE the mutated child; never recurse
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../target"))
        .join("mutate-four-outputs");
    let out = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "test",
            "--locked",
            "-p",
            "forge-tests",
            "--features",
            "mutate-four-outputs",
            "--test",
            "test_four_outputs_agree",
            "--",
            "--exact",
            "four_outputs_agree_for_every_item",
        ])
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(&root)
        .output()
        .expect("spawn cargo");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "the mutate-drift build PASSED the guard — the guard is vacuous:\n{text}"
    );
    assert!(
        text.contains(DRIFT_MARKER) && text.contains("REFLECT-0006"),
        "the mutate-drift build failed for another reason (it must fail on drift):\n{text}"
    );
}
