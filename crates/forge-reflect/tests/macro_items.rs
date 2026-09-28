//! `#[forge_api]` on representative items, in a crate that depends on `bevy_reflect`
//! directly (forge-tests' guard covers the crate that does not): purity inference, titles,
//! nested types, validation refusals, registry semantics.

use forge_core::{CodedError, EntityId};
use forge_reflect::{
    ForgeApi, ForgeRegistry, ItemKind, PinKind, Purity, ReflectError, check_agreement, forge_api,
};

/// A 2-D size.
#[forge_api(name = "Size (2D)")]
#[derive(Clone, Debug, PartialEq)]
pub struct Size2 {
    /// Width.
    #[forge(units = "m", min = 0)]
    pub w: f64,
    /// Height.
    #[forge(units = "m", min = 0)]
    pub h: f64,
}

/// A labelled panel: a nested `#[forge_api]` struct field and a text field.
#[forge_api(category = "UI")]
#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    /// What it says.
    pub label: String,
    /// How big it is.
    pub size: Size2,
    /// Stacking order.
    #[forge(min = -10, max = 10, step = 1)]
    pub layer: i32,
}

/// Area of a size.
#[forge_api(returns(units = "m²"))]
pub fn area(size: Size2) -> f64 {
    size.w * size.h
}

/// Does the entity exist? (Reads the world.)
#[forge_api]
pub fn is_alive(world: &forge_core::World, #[forge(entity)] target: EntityId) -> bool {
    world.contains(target)
}

/// Despawns an entity: mutating and destructive.
#[forge_api(mutates, destructive, category = "World")]
pub fn despawn(
    world: &mut forge_core::World,
    #[forge(entity, doc = "Who goes.")] target: EntityId,
) -> forge_core::Result<()> {
    Ok(world.despawn(target)?)
}

/// Units on a boolean: compiles (the macro cannot see types), refused at registration.
#[forge_api]
pub fn bad_units(#[forge(units = "m")] flag: bool) -> bool {
    flag
}

/// A range on text: refused at registration.
#[forge_api]
pub fn bad_range(#[forge(min = 0)] name: String) -> String {
    name
}

/// `entity` on a number: refused at registration.
#[forge_api]
pub fn bad_entity(#[forge(entity)] x: f64) -> f64 {
    x
}

fn code(e: &ReflectError) -> &'static str {
    e.error_code().as_str()
}

#[test]
fn purity_is_inferred_from_context_parameters() {
    let a = area::describe();
    assert_eq!(a.node.purity, Purity::Pure);
    assert!(a.node.context.is_empty() && a.command.is_none());
    let c = is_alive::describe();
    assert_eq!(c.node.purity, Purity::Read);
    assert_eq!(c.node.inputs.len(), 1, "the context parameter is not a pin");
    assert_eq!(c.node.outputs[0].kind, PinKind::Bool);
    let d = despawn::describe();
    assert_eq!(d.node.purity, Purity::Mutate);
    let cmd = d.command.as_ref().expect("command");
    assert!(cmd.destructive && cmd.fallible);
    assert_eq!(d.schema["x-forge-destructive"], true);
    for item in [a, c, d] {
        check_agreement(&item).expect("agrees");
    }
}

#[test]
fn titles_nested_types_and_ranges() {
    let mut reg = ForgeRegistry::new();
    reg.register::<Panel>().expect("panel");
    reg.register::<area>().expect("area");
    let p = reg
        .items()
        .find(|i| i.ident == "Panel")
        .expect("panel")
        .clone();
    assert_eq!(p.kind, ItemKind::Struct);
    assert_eq!(p.node.display_name, "Panel");
    assert_eq!(p.node.inputs[1].kind, PinKind::Struct);
    assert_eq!(p.node.inputs[2].meta.min, Some(-10.0));
    assert_eq!(p.schema["properties"]["layer"]["minimum"], -10.0);
    // The nested struct is defined once in the document and referenced.
    let doc = reg.schema_document();
    let size = doc["$defs"]
        .as_object()
        .expect("defs")
        .iter()
        .find(|(k, _)| k.ends_with("::Size2"))
        .expect("Size2 defined")
        .1;
    assert_eq!(size["title"], "Size (2D)");
    assert_eq!(size["properties"]["w"]["x-forge-unit"], "m");
    // And registered with bevy_reflect, so the serialiser can walk it.
    assert!(
        reg.type_registry()
            .iter()
            .any(|t| t.type_info().type_path().ends_with("::Size2"))
    );
    let area = reg.items().find(|i| i.ident == "area").expect("area");
    assert_eq!(area.node.display_name, "Area");
    assert_eq!(area.node.outputs[0].meta.units, Some("m²"));
}

#[test]
fn annotations_on_the_wrong_kind_are_refused() {
    let mut reg = ForgeRegistry::new();
    for (e, attr) in [
        (
            reg.register::<bad_units>()
                .map(|_| ())
                .expect_err("units on bool"),
            "units",
        ),
        (
            reg.register::<bad_range>()
                .map(|_| ())
                .expect_err("range on text"),
            "range",
        ),
        (
            reg.register::<bad_entity>()
                .map(|_| ())
                .expect_err("entity on f64"),
            "entity",
        ),
    ] {
        assert_eq!(code(&e), "REFLECT-0002", "{e}");
        assert!(
            matches!(e, ReflectError::MetaOnWrongKind { attr: a, .. } if a == attr),
            "{e}"
        );
    }
    assert_eq!(reg.items().len(), 0, "refused items are not registered");
}

#[test]
fn registry_is_idempotent_and_refuses_impostors() {
    let mut reg = ForgeRegistry::new();
    reg.register::<Size2>().expect("first");
    reg.register::<Size2>().expect("again: idempotent");
    assert_eq!(reg.items().len(), 1);
    // A different, self-consistent item claiming the same path.
    let mut impostor = Size2::describe();
    impostor.node.display_name = "Impostor";
    impostor.schema["title"] = serde_json::json!("Impostor");
    check_agreement(&impostor).expect("self-consistent");
    let e = reg
        .insert(impostor)
        .map(|_| ())
        .expect_err("duplicate path");
    assert_eq!(code(&e), "REFLECT-0004");
    let e = reg.item("no::such::item").map(|_| ()).expect_err("unknown");
    assert_eq!(code(&e), "REFLECT-0005");
    // Errors convert into the engine error with their code.
    let err: forge_core::Error = e.into();
    assert_eq!(err.code().as_str(), "REFLECT-0005");
}

#[test]
fn the_annotated_fns_still_run() {
    assert_eq!(area(Size2 { w: 2.0, h: 3.0 }), 6.0);
    let mut w = forge_core::World::new();
    let e = w.spawn(());
    assert!(is_alive(&w, e));
    despawn(&mut w, e).expect("despawn");
    assert!(!is_alive(&w, e));
    assert!(bad_units(true));
    assert_eq!(bad_range("x".into()), "x");
    assert_eq!(bad_entity(1.0), 1.0);
}
