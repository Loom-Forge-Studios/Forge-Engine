//! `check_agreement` — the Ch.6 guard's engine: do an item's four outputs describe the same
//! thing? "A drift here is silent and catastrophic."
//!
//! The `bevy_reflect` side is the independent witness: its `TypeInfo` comes from the
//! compiler (`<T as Typed>` for every type in the signature) and from `bevy_reflect`'s own
//! derive (field names, order and doc comments), not from forge-reflect's code. Everything
//! else is compared against it and against each other:
//!
//! | checked | reflect | node | schema | command |
//! |---|---|---|---|---|
//! | arity, names, order | ✓ | ✓ | ✓ (`required`) | ✓ |
//! | type identity | `type_path` | `type_path` | `x-forge-type` | `type_path` |
//! | value shape | `TypeInfo` kind | `PinKind` | resolved `type`/`oneOf` | — |
//! | units, range, step, labels, flags | — | `FieldMeta` | keywords | `FieldMeta` |
//! | docs | `docs()` (struct/enum) | `doc` | `description` | `doc` |
//! | purity / command | — | `purity` | `x-forge-purity`, `x-forge-command` | present iff mutating |
//! | return | `TypeInfo` | output pin | `x-forge-returns` | — |

use bevy_reflect::TypeInfo;
use bevy_reflect::enums::VariantInfo;
use serde_json::Value;

use crate::desc::{ApiItem, ItemKind, Pin, Purity, ReflectDesc, normalize_doc, upper_camel};
use crate::{PinKind, ReflectError, SchemaDefs};

/// Every disagreement between `item`'s outputs; `Err(REFLECT-0006)` listing them all.
pub fn check_agreement(item: &ApiItem) -> Result<(), ReflectError> {
    let mut c = Checker {
        problems: Vec::new(),
        defs: &item.schema_defs,
    };
    match (&item.kind, &item.reflect) {
        (ItemKind::Fn, ReflectDesc::Fn { args, ret }) => c.function(item, args, *ret),
        (ItemKind::Struct, ReflectDesc::Type(info)) => c.structure(item, info),
        (ItemKind::Enum, ReflectDesc::Type(info)) => c.enumeration(item, info),
        (k, _) => c.fail(format!(
            "{k:?} item carries the wrong kind of reflect descriptor"
        )),
    }
    if c.problems.is_empty() {
        Ok(())
    } else {
        Err(ReflectError::Drift {
            item: item.path.clone(),
            problems: c.problems,
        })
    }
}

struct Checker<'a> {
    problems: Vec<String>,
    defs: &'a SchemaDefs,
}

fn str_at<'v>(v: &'v Value, key: &str) -> Option<&'v str> {
    v.get(key).and_then(Value::as_str)
}

/// The names in a schema object's `required` (declaration order).
fn required(v: &Value) -> Vec<&str> {
    v.get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// What JSON shape a reflected type has, when `bevy_reflect` alone can tell.
fn reflect_shape(info: &TypeInfo) -> Option<&'static str> {
    match info {
        TypeInfo::Struct(_) => Some("object"),
        TypeInfo::Enum(_) => Some("oneOf"),
        TypeInfo::Opaque(_) => match info.type_path() {
            "f64" | "f32" => Some("number"),
            "bool" => Some("boolean"),
            "alloc::string::String" | "std::string::String" => Some("string"),
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
            | "u128" | "usize" => Some("integer"),
            // An engine type reflected opaque below forge-reflect: its shape is its
            // ForgeType's to define (checked against the pin kind instead).
            _ => None,
        },
        _ => None,
    }
}

fn kind_shape(k: PinKind) -> &'static str {
    match k {
        PinKind::Bool => "boolean",
        PinKind::Int | PinKind::Entity | PinKind::Id | PinKind::Time => "integer",
        PinKind::Float => "number",
        PinKind::Text => "string",
        PinKind::Vector | PinKind::Struct => "object",
        PinKind::Enum => "oneOf",
    }
}

impl Checker<'_> {
    fn fail(&mut self, p: String) {
        self.problems.push(p);
    }

    fn eq<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, a: (&str, T), b: (&str, T)) {
        if a.1 != b.1 {
            self.fail(format!(
                "{what}: {} says {:?}, {} says {:?}",
                a.0, a.1, b.0, b.1
            ));
        }
    }

    fn schema_shape<'v>(&'v self, s: &'v Value) -> Option<&'v str> {
        let r = self.defs.resolve(s);
        if r.get("oneOf").is_some() {
            Some("oneOf")
        } else {
            str_at(r, "type")
        }
    }

    /// Pin vs schema property: identity, shape and every annotation keyword.
    fn pin_vs_schema(&mut self, at: &str, pin: &Pin, s: &Value) {
        let m = &pin.meta;
        self.eq(
            &format!("{at} type"),
            ("node", Some(pin.type_path)),
            ("schema", str_at(s, "x-forge-type")),
        );
        let shape = self.schema_shape(s).map(str::to_string);
        self.eq(
            &format!("{at} shape"),
            ("node kind", Some(kind_shape(pin.kind))),
            ("schema", shape.as_deref()),
        );
        self.eq(
            &format!("{at} title"),
            ("node", Some(m.display_name)),
            ("schema", str_at(s, "title")),
        );
        let doc = (!m.doc.is_empty()).then_some(m.doc);
        self.eq(
            &format!("{at} tooltip"),
            ("node", doc),
            ("schema", str_at(s, "description")),
        );
        self.eq(
            &format!("{at} units"),
            ("node", m.units),
            ("schema", str_at(s, "x-forge-unit")),
        );
        self.eq(
            &format!("{at} unit parse"),
            ("node", m.units.is_some()),
            ("parsed", pin.unit.is_some()),
        );
        if m.min.is_some() {
            self.eq(
                &format!("{at} min"),
                ("node", m.min),
                ("schema", s.get("minimum").and_then(Value::as_f64)),
            );
        }
        if m.max.is_some() {
            self.eq(
                &format!("{at} max"),
                ("node", m.max),
                ("schema", s.get("maximum").and_then(Value::as_f64)),
            );
        }
        self.eq(
            &format!("{at} step"),
            ("node", m.step),
            ("schema", s.get("x-forge-step").and_then(Value::as_f64)),
        );
        let flag = |k: &str| s.get(k).and_then(Value::as_bool).unwrap_or(false);
        self.eq(
            &format!("{at} read_only"),
            ("node", m.read_only),
            ("schema", flag("readOnly")),
        );
        self.eq(
            &format!("{at} hidden"),
            ("node", m.hidden),
            ("schema", flag("x-forge-hidden")),
        );
        self.eq(
            &format!("{at} entity"),
            ("node", m.entity),
            ("schema", flag("x-forge-entity")),
        );
        self.eq(
            &format!("{at} category"),
            ("node", m.category),
            ("schema", str_at(s, "x-forge-category")),
        );
        self.eq(
            &format!("{at} widget"),
            ("node", m.widget),
            ("schema", str_at(s, "x-forge-widget")),
        );
    }

    /// Reflected type vs pin: identity and shape.
    fn reflect_vs_pin(&mut self, at: &str, info: &TypeInfo, pin: &Pin) {
        self.eq(
            &format!("{at} type"),
            ("reflect", info.type_path()),
            ("node", pin.type_path),
        );
        if let Some(shape) = reflect_shape(info) {
            self.eq(
                &format!("{at} shape"),
                ("reflect", shape),
                ("node kind", kind_shape(pin.kind)),
            );
        }
    }

    /// A list of pins against a schema object (`properties` + `required`) and, optionally,
    /// reflected `(name, type, docs)` fields.
    fn fields(
        &mut self,
        at: &str,
        reflect: Option<&[(&'static str, &'static TypeInfo, Option<&str>)]>,
        pins: &[Pin],
        schema: &Value,
    ) {
        let req = required(schema);
        let props = schema.get("properties").and_then(Value::as_object);
        let n_props = props.map_or(0, serde_json::Map::len);
        self.eq(
            &format!("{at} arity"),
            ("node", pins.len()),
            ("schema required", req.len()),
        );
        self.eq(
            &format!("{at} arity"),
            ("node", pins.len()),
            ("schema properties", n_props),
        );
        let pin_names: Vec<&str> = pins.iter().map(|p| p.name).collect();
        self.eq(
            &format!("{at} names"),
            ("node", pin_names.clone()),
            ("schema", req.clone()),
        );
        if let Some(r) = reflect {
            self.eq(
                &format!("{at} arity"),
                ("reflect", r.len()),
                ("node", pins.len()),
            );
            let rn: Vec<&str> = r.iter().map(|f| f.0).collect();
            self.eq(&format!("{at} names"), ("reflect", rn), ("node", pin_names));
            for (f, pin) in r.iter().zip(pins) {
                self.reflect_vs_pin(&format!("{at}.{}", f.0), f.1, pin);
                if let Some(d) = f.2 {
                    self.eq(
                        &format!("{at}.{} doc", f.0),
                        ("reflect", normalize_doc(d)),
                        ("node", pin.meta.doc.to_string()),
                    );
                }
            }
        }
        for pin in pins {
            match props.and_then(|p| p.get(pin.name)) {
                Some(s) => self.pin_vs_schema(&format!("{at}.{}", pin.name), pin, s),
                None => self.fail(format!(
                    "{at}.{}: in the node, missing from the schema",
                    pin.name
                )),
            }
        }
    }

    fn item_doc(&mut self, item: &ApiItem, reflect_doc: Option<&str>) {
        let node = item.node.doc;
        self.eq("doc", ("node", Some(node)), ("item", Some(item.doc)));
        let schema = str_at(&item.schema, "description").unwrap_or("");
        self.eq("doc", ("node", node), ("schema", schema));
        self.eq(
            "title",
            ("node", Some(item.node.display_name)),
            ("schema", str_at(&item.schema, "title")),
        );
        if let Some(d) = reflect_doc {
            self.eq(
                "doc",
                ("reflect", normalize_doc(d)),
                ("node", node.to_string()),
            );
        }
        self.eq(
            "category",
            ("node", item.node.category),
            ("schema", str_at(&item.schema, "x-forge-category")),
        );
    }

    fn function(
        &mut self,
        item: &ApiItem,
        args: &[crate::ReflectArg],
        ret: Option<&'static TypeInfo>,
    ) {
        let node = &item.node;
        self.item_doc(item, None);
        let reflect: Vec<_> = args.iter().map(|a| (a.name, a.info, None)).collect();
        self.fields("input", Some(&reflect), &node.inputs, &item.schema);

        // Purity and the command variant.
        self.eq(
            "purity",
            ("node", Some(node.purity.name())),
            ("schema", str_at(&item.schema, "x-forge-purity")),
        );
        let mutates = node.purity == Purity::Mutate;
        let has_mut_ctx = node.context.iter().any(|c| c.mutable);
        self.eq(
            "purity",
            ("node says mutate", mutates),
            ("context has &mut", has_mut_ctx),
        );
        if node.purity == Purity::Pure && !node.context.is_empty() {
            self.fail("purity: pure node with a context parameter".into());
        }
        self.eq(
            "command",
            ("node mutates", mutates),
            ("command present", item.command.is_some()),
        );
        let variant = item.command.as_ref().map(|c| c.variant.clone());
        self.eq(
            "command variant",
            ("command", variant.as_deref()),
            ("schema", str_at(&item.schema, "x-forge-command")),
        );
        if let Some(cmd) = &item.command {
            self.eq(
                "command variant",
                ("command", cmd.variant.clone()),
                ("fn", upper_camel(item.ident)),
            );
            self.eq(
                "command target",
                ("command", cmd.target.as_str()),
                ("node", node.path.as_str()),
            );
            self.eq("command doc", ("command", cmd.doc), ("node", node.doc));
            self.eq(
                "command destructive",
                ("command", cmd.destructive),
                ("node", node.destructive),
            );
            self.eq(
                "command fallible",
                ("command", cmd.fallible),
                ("node", node.fallible),
            );
            let cn: Vec<&str> = cmd.fields.iter().map(|p| p.name).collect();
            let nn: Vec<&str> = node.inputs.iter().map(|p| p.name).collect();
            self.eq("command fields", ("command", cn), ("node", nn));
            for (c, n) in cmd.fields.iter().zip(&node.inputs) {
                self.eq(
                    &format!("command.{} type", c.name),
                    ("command", c.type_path),
                    ("node", n.type_path),
                );
                self.eq(
                    &format!("command.{} meta", c.name),
                    ("command", &c.meta),
                    ("node", &n.meta),
                );
            }
        }
        let fallible = item.schema.get("x-forge-fallible").and_then(Value::as_bool);
        self.eq(
            "fallible",
            ("node", Some(node.fallible)),
            ("schema", fallible),
        );
        let destructive = item
            .schema
            .get("x-forge-destructive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.eq(
            "destructive",
            ("node", node.destructive),
            ("schema", destructive),
        );
        if node.destructive && !mutates {
            self.fail("destructive: only a mutating fn can be destructive".into());
        }

        // Return value.
        let sret = item.schema.get("x-forge-returns");
        self.eq(
            "outputs",
            ("reflect", usize::from(ret.is_some())),
            ("node", node.outputs.len()),
        );
        self.eq(
            "outputs",
            ("node", node.outputs.len()),
            ("schema", usize::from(sret.is_some())),
        );
        if let (Some(info), Some(pin)) = (ret, node.outputs.first()) {
            self.reflect_vs_pin("return", info, pin);
        }
        if let (Some(pin), Some(s)) = (node.outputs.first(), sret) {
            self.pin_vs_schema("return", pin, s);
        }
    }

    fn self_output(&mut self, item: &ApiItem, info: &TypeInfo) {
        let outs: Vec<&str> = item.node.outputs.iter().map(|p| p.type_path).collect();
        self.eq(
            "output",
            ("reflect", vec![info.type_path()]),
            ("node", outs),
        );
        self.eq(
            "type",
            ("reflect", Some(info.type_path())),
            ("schema", str_at(&item.schema, "x-forge-type")),
        );
        self.eq(
            "path",
            ("reflect", info.type_path()),
            ("item", item.path.as_str()),
        );
    }

    fn structure(&mut self, item: &ApiItem, info: &TypeInfo) {
        let TypeInfo::Struct(s) = info else {
            self.fail(format!(
                "reflect says {:?}, the node says struct",
                info.kind()
            ));
            return;
        };
        self.item_doc(item, s.docs());
        self.self_output(item, info);
        let reflect: Vec<_> = s
            .iter()
            .filter_map(|f| f.type_info().map(|t| (f.name(), t, f.docs())))
            .collect();
        if reflect.len() != s.field_len() {
            self.fail("reflect: a field has no static TypeInfo".into());
        }
        self.fields("field", Some(&reflect), &item.node.inputs, &item.schema);
    }

    fn enumeration(&mut self, item: &ApiItem, info: &TypeInfo) {
        let TypeInfo::Enum(e) = info else {
            self.fail(format!(
                "reflect says {:?}, the node says enum",
                info.kind()
            ));
            return;
        };
        self.item_doc(item, e.docs());
        self.self_output(item, info);
        let one_of = item
            .schema
            .get("oneOf")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let rn: Vec<&str> = e.variant_names().to_vec();
        let nn: Vec<&str> = item.node.variants.iter().map(|v| v.name).collect();
        let sn: Vec<&str> = one_of
            .iter()
            .filter_map(|v| str_at(v, "x-forge-variant"))
            .collect();
        self.eq("variants", ("reflect", rn.clone()), ("node", nn));
        self.eq("variants", ("reflect", rn), ("schema", sn));
        for ((rv, nv), sv) in e.iter().zip(&item.node.variants).zip(&one_of) {
            let at = format!("variant {}", rv.name());
            self.eq(
                &format!("{at} title"),
                ("node", Some(nv.display_name)),
                ("schema", str_at(sv, "title")),
            );
            let ndoc = (!nv.doc.is_empty()).then_some(nv.doc);
            self.eq(
                &format!("{at} doc"),
                ("node", ndoc),
                ("schema", str_at(sv, "description")),
            );
            if let Some(d) = rv.docs() {
                self.eq(
                    &format!("{at} doc"),
                    ("reflect", normalize_doc(d)),
                    ("node", nv.doc.to_string()),
                );
            }
            match rv {
                VariantInfo::Unit(_) => {
                    if !nv.fields.is_empty() {
                        self.fail(format!("{at}: reflect says unit, the node has fields"));
                    }
                    self.eq(
                        &format!("{at} const"),
                        ("reflect", Some(rv.name())),
                        ("schema", str_at(sv, "const")),
                    );
                }
                VariantInfo::Struct(sv_info) => {
                    let reflect: Vec<_> = sv_info
                        .iter()
                        .filter_map(|f| f.type_info().map(|t| (f.name(), t, f.docs())))
                        .collect();
                    let body = sv
                        .get("properties")
                        .and_then(|p| p.get(rv.name()))
                        .cloned()
                        .unwrap_or(Value::Null);
                    self.fields(&at, Some(&reflect), &nv.fields, &body);
                }
                VariantInfo::Tuple(_) => {
                    self.fail(format!("{at}: tuple variants are not supported"))
                }
            }
        }
    }
}
