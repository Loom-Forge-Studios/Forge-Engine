//! Where the graph editor's nodes come from: **the `#[forge_api]` registry** (Ch.24.2 —
//! "every `#[forge_api]` item is a node"). [`NodeLibrary`] turns every registered item's
//! node descriptor (Ch.6 output 2: pins, types, units, docs, purity) into a node the search
//! offers; nothing here is written per node.
//!
//! Until the engine's subsystems register their own items, the editor registers the small
//! set of real `#[forge_api]` functions below (math, logic, physics, material, generator,
//! PCG) plus the component catalogue's structs and enums (make / select nodes). Each is an
//! ordinary Rust function the generated code calls (Ch.24.1: a graph compiles).
//!
//! **Which graphs a node belongs to** is read from its category: `Math` in every graph,
//! `Logic` / `Physics` / `Gameplay` and uncategorised items in blueprints, `Material` in
//! material graphs, `Generator` in generator graphs, `PCG` in PCG graphs. A mutating item
//! (a command) is a blueprint node only: the other three graphs are pure (Ch.10, 13, 14).

use std::collections::BTreeMap;

use forge_reflect::{ForgeRegistry, Pin, PinKind, Purity, forge_api};

use super::GraphKind;
use crate::EditorError;

/// The built-in reroute's op (not a registry item: a wire's waypoint, typed by its source).
pub const REROUTE: &str = "forge.reroute";

/// One node the library offers.
#[derive(Clone, Debug, PartialEq)]
pub struct LibNode {
    /// The op stored in the graph (the item's path).
    pub op: String,
    /// The item's identifier (what generated code calls).
    pub ident: String,
    pub title: String,
    pub category: String,
    /// The node's tooltip (the doc comment).
    pub doc: String,
    pub inputs: Vec<Pin>,
    pub outputs: Vec<Pin>,
    pub purity: Purity,
    /// Context parameters the call takes first (`world`).
    pub context: Vec<String>,
    pub kinds: Vec<GraphKind>,
    /// The same kinds as the IR names them (`forge_graph`).
    pub ir_kinds: Vec<forge_graph::GraphKind>,
}

impl LibNode {
    pub fn input(&self, name: &str) -> Option<&Pin> {
        self.inputs.iter().find(|p| p.name == name)
    }
    pub fn output(&self, name: &str) -> Option<&Pin> {
        self.outputs.iter().find(|p| p.name == name)
    }
    pub fn allowed_in(&self, k: GraphKind) -> bool {
        self.kinds.contains(&k)
    }
}

/// The nodes (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct NodeLibrary {
    nodes: BTreeMap<String, LibNode>,
    /// ident → op, for idents no two items share (generated text calls those by ident).
    by_ident: BTreeMap<String, Option<String>>,
}

/// The graph kinds a category's items belong to: the IR's rule
/// (`forge_graph::GraphKind::for_category`).
fn kinds_for(category: &str, purity: Purity) -> Vec<forge_graph::GraphKind> {
    forge_graph::GraphKind::for_category(category, purity == Purity::Mutate)
}

fn editor_kind(k: forge_graph::GraphKind) -> GraphKind {
    match k {
        forge_graph::GraphKind::Blueprint => GraphKind::Blueprint,
        forge_graph::GraphKind::Material => GraphKind::Material,
        forge_graph::GraphKind::Generator => GraphKind::Generator,
        forge_graph::GraphKind::Pcg => GraphKind::Pcg,
    }
}

impl NodeLibrary {
    /// Every item of every registry is a node.
    pub fn from_registries(regs: &[&ForgeRegistry]) -> Self {
        let mut lib = Self::default();
        for reg in regs {
            for item in reg.items() {
                let n = &item.node;
                let category = n.category.unwrap_or("Blueprint").to_string();
                let node = LibNode {
                    op: n.path.clone(),
                    ident: item.ident.to_string(),
                    title: n.display_name.to_string(),
                    category: category.clone(),
                    doc: n.doc.to_string(),
                    inputs: n.inputs.clone(),
                    outputs: n.outputs.clone(),
                    purity: n.purity,
                    context: n.context.iter().map(|c| c.name.to_string()).collect(),
                    kinds: kinds_for(&category, n.purity)
                        .into_iter()
                        .map(editor_kind)
                        .collect(),
                    ir_kinds: kinds_for(&category, n.purity),
                };
                lib.insert(node);
            }
        }
        lib
    }

    fn insert(&mut self, node: LibNode) {
        let e = self.by_ident.entry(node.ident.clone()).or_insert(None);
        *e = match e {
            None if !self.nodes.values().any(|n| n.ident == node.ident) => Some(node.op.clone()),
            _ => None,
        };
        self.nodes.insert(node.op.clone(), node);
    }

    /// The built-in library (the functions in this module) over `extra` registries (the
    /// component catalogue's).
    pub fn builtin(extra: &[&ForgeRegistry]) -> Result<Self, EditorError> {
        let reg = builtin_registry()?;
        // WP-23: a generator graph's stages (Ch.13) come in `extra`, from
        // the generator backend a plugin provides (`services::EditorServices::generator`).
        let mut all: Vec<&ForgeRegistry> = vec![&reg];
        all.extend_from_slice(extra);
        Ok(Self::from_registries(&all))
    }

    pub fn get(&self, op: &str) -> Option<&LibNode> {
        self.nodes.get(op)
    }
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = &LibNode> {
        self.nodes.values()
    }
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    /// The op an identifier in generated text calls (`None`: unknown or ambiguous).
    pub fn op_of_ident(&self, ident: &str) -> Option<&str> {
        if let Some((k, _)) = self.nodes.get_key_value(ident) {
            return Some(k.as_str());
        }
        self.by_ident.get(ident)?.as_deref()
    }
    /// What generated text calls `op` by (its identifier when unambiguous, else its path).
    pub fn call_name<'a>(&'a self, op: &'a str) -> &'a str {
        match self.nodes.get(op) {
            Some(n) if self.by_ident.get(&n.ident).is_some_and(Option::is_some) => &n.ident,
            _ => op,
        }
    }
    /// The nodes a graph of `kind` may use, in category then title order.
    pub fn for_kind(&self, kind: GraphKind) -> Vec<&LibNode> {
        let mut v: Vec<&LibNode> = self.nodes.values().filter(|n| n.allowed_in(kind)).collect();
        v.sort_by(|a, b| (&a.category, &a.title).cmp(&(&b.category, &b.title)));
        v
    }
}

/// A pin's type and unit as the canvas shows it (`f64 · m/s`).
pub fn pin_detail(p: &Pin) -> String {
    let ty = p.type_path.rsplit("::").next().unwrap_or(p.type_path);
    match &p.unit {
        Some(u) => format!("{ty} · {}", u.text()),
        None => ty.to_string(),
    }
}

/// A pin's colour on the canvas (one per kind of value).
pub fn pin_role(k: PinKind) -> forge_ui::ColorRole {
    use forge_ui::ColorRole as C;
    match k {
        PinKind::Float => C::Accent,
        PinKind::Int => C::Success,
        PinKind::Bool => C::Warning,
        PinKind::Vector => C::FocusRing,
        PinKind::Entity | PinKind::Id => C::Danger,
        PinKind::Text | PinKind::Time => C::FgMuted,
        PinKind::Struct | PinKind::Enum => C::FgPrimary,
    }
}

/// A registry holding the built-in nodes.
pub fn builtin_registry() -> Result<ForgeRegistry, EditorError> {
    let err = |e: forge_reflect::ReflectError| EditorError::Settings(e.to_string());
    let mut r = ForgeRegistry::new();
    r.register::<add>().map_err(err)?;
    r.register::<subtract>().map_err(err)?;
    r.register::<multiply>().map_err(err)?;
    r.register::<lerp>().map_err(err)?;
    r.register::<clamp>().map_err(err)?;
    r.register::<greater>().map_err(err)?;
    r.register::<select>().map_err(err)?;
    r.register::<distance>().map_err(err)?;
    r.register::<speed>().map_err(err)?;
    r.register::<kinetic_energy>().map_err(err)?;
    r.register::<momentum>().map_err(err)?;
    r.register::<speed_from_impulse>().map_err(err)?;
    r.register::<schlick_fresnel>().map_err(err)?;
    r.register::<saturate>().map_err(err)?;
    r.register::<value_noise>().map_err(err)?;
    r.register::<terrace>().map_err(err)?;
    r.register::<scatter_count>().map_err(err)?;
    r.register::<slope_mask>().map_err(err)?;
    Ok(r)
}

// ---- the built-in nodes ----------------------------------------------------------------------
//
// Real functions: the stub IR's generated text calls them, and they are what a compiled graph
// runs until the engine's own items replace them.

/// The sum of two numbers.
#[forge_api(category = "Math")]
pub fn add(a: f64, b: f64) -> f64 {
    a + b
}

/// The difference of two numbers.
#[forge_api(category = "Math")]
pub fn subtract(a: f64, b: f64) -> f64 {
    a - b
}

/// The product of two numbers.
#[forge_api(category = "Math")]
pub fn multiply(a: f64, b: f64) -> f64 {
    a * b
}

/// Linear interpolation from `a` to `b` by `t`.
#[forge_api(category = "Math")]
pub fn lerp(
    a: f64,
    b: f64,
    #[forge(min = 0.0, max = 1.0, doc = "0 gives a, 1 gives b.")] t: f64,
) -> f64 {
    a + (b - a) * t
}

/// A value kept between a lower and an upper bound.
#[forge_api(category = "Math")]
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

/// Whether `a` is greater than `b`.
#[forge_api(category = "Logic")]
pub fn greater(a: f64, b: f64) -> bool {
    a > b
}

/// One of two values, chosen by a condition.
#[forge_api(category = "Logic")]
pub fn select(condition: bool, if_true: f64, if_false: f64) -> f64 {
    if condition { if_true } else { if_false }
}

/// The distance covered at a constant speed.
#[forge_api(category = "Physics", returns(units = "m"))]
pub fn distance(#[forge(units = "m/s")] speed: f64, #[forge(units = "s")] time: f64) -> f64 {
    speed * time
}

/// The constant speed that covers a distance in a time.
#[forge_api(category = "Physics", returns(units = "m/s"))]
pub fn speed(#[forge(units = "m")] distance: f64, #[forge(units = "s")] time: f64) -> f64 {
    if time == 0.0 { 0.0 } else { distance / time }
}

/// Kinetic energy of a moving mass.
#[forge_api(category = "Physics", returns(units = "J"))]
pub fn kinetic_energy(
    #[forge(units = "kg", min = 0.0)] mass: f64,
    #[forge(units = "m/s")] speed: f64,
) -> f64 {
    0.5 * mass * speed * speed
}

/// Momentum of a moving mass (the impulse that would stop it).
#[forge_api(category = "Physics", returns(units = "N·s"))]
pub fn momentum(
    #[forge(units = "kg", min = 0.0)] mass: f64,
    #[forge(units = "m/s")] velocity: f64,
) -> f64 {
    mass * velocity
}

/// The speed an impulse gives a mass at rest.
#[forge_api(category = "Physics", returns(units = "m/s"))]
pub fn speed_from_impulse(
    #[forge(units = "N·s")] impulse: f64,
    #[forge(units = "kg", min = 0.0)] mass: f64,
) -> f64 {
    if mass == 0.0 { 0.0 } else { impulse / mass }
}

/// Schlick's approximation of Fresnel reflectance.
#[forge_api(category = "Material")]
pub fn schlick_fresnel(
    #[forge(min = 0.0, max = 1.0, doc = "Reflectance at normal incidence.")] f0: f64,
    #[forge(min = 0.0, max = 1.0, doc = "Cosine of the view angle.")] cos_theta: f64,
) -> f64 {
    let m = (1.0 - cos_theta.clamp(0.0, 1.0)).powi(5);
    f0 + (1.0 - f0) * m
}

/// A value clamped to 0..=1.
#[forge_api(category = "Material")]
pub fn saturate(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// Smooth value noise in -1..=1 at a point on the ground, deterministic in its seed.
#[forge_api(category = "Generator")]
pub fn value_noise(seed: u64, #[forge(units = "m")] x: f64, #[forge(units = "m")] y: f64) -> f64 {
    let h = |ix: i64, iy: i64| -> f64 {
        let mut v = seed
            ^ (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        v ^= v >> 33;
        v = v.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        v ^= v >> 33;
        (v >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i64, fy as i64);
    let (tx, ty) = (x - fx, y - fy);
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(tx), s(ty));
    let a = h(ix, iy) + (h(ix + 1, iy) - h(ix, iy)) * sx;
    let b = h(ix, iy + 1) + (h(ix + 1, iy + 1) - h(ix, iy + 1)) * sx;
    a + (b - a) * sy
}

/// Heights snapped to terraces of a step height.
#[forge_api(category = "Generator", returns(units = "m"))]
pub fn terrace(
    #[forge(units = "m")] height: f64,
    #[forge(units = "m", min = 0.0)] step: f64,
) -> f64 {
    if step <= 0.0 {
        height
    } else {
        (height / step).floor() * step
    }
}

/// How many instances a density scatters over an area.
#[forge_api(category = "PCG")]
pub fn scatter_count(
    #[forge(units = "m²", min = 0.0)] area: f64,
    #[forge(units = "1/m²", min = 0.0)] density: f64,
) -> u64 {
    (area * density).max(0.0).round() as u64
}

/// 1 on ground flatter than a slope limit, 0 on steeper ground.
#[forge_api(category = "PCG")]
pub fn slope_mask(#[forge(units = "deg")] slope: f64, #[forge(units = "deg")] limit: f64) -> f64 {
    if slope <= limit { 1.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_is_a_node_with_its_units_and_graphs() {
        let lib = NodeLibrary::builtin(&[]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            lib.len(),
            18,
            "18 built-in nodes (the generator stages come from a backend plugin)"
        );
        let ke = lib
            .nodes()
            .find(|n| n.ident == "kinetic_energy")
            .unwrap_or_else(|| panic!("kinetic_energy"));
        assert_eq!(ke.inputs[1].unit.as_ref().map(|u| u.text()), Some("m/s"));
        assert_eq!(ke.outputs[0].name, "return");
        assert_eq!(ke.kinds, vec![GraphKind::Blueprint]);
        assert_eq!(lib.op_of_ident("kinetic_energy"), Some(ke.op.as_str()));
        assert_eq!(lib.call_name(&ke.op), "kinetic_energy");
        let add = lib
            .nodes()
            .find(|n| n.ident == "add")
            .map(|n| n.kinds.clone());
        assert_eq!(add, Some(GraphKind::ALL.to_vec()));
        assert!(
            lib.for_kind(GraphKind::Material)
                .iter()
                .all(|n| n.category == "Math" || n.category == "Material")
        );
        assert_eq!(pin_detail(&ke.inputs[1]), "f64 · m/s");
        assert_eq!(value_noise(7, 1.5, 2.25), value_noise(7, 1.5, 2.25));
        assert!(value_noise(7, 1.5, 2.25).abs() <= 1.0);
    }
}
