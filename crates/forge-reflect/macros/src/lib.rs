//! `#[forge_api]` (Ch.6). Use it through `forge_reflect::forge_api`; the generated code
//! names `::forge_reflect`, so a crate using it depends on `forge-reflect`.
//!
//! One parse of the item; the macro then emits the `bevy_reflect` view, the node pins, the
//! schema properties and the command fields as **separate token lists**, so
//! `forge_reflect::check_agreement` is comparing four real outputs, not one list four times.

#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro2::{Literal, Span, TokenStream as TokenStream2};
use quote::{ToTokens, quote};
use syn::meta::ParseNestedMeta;
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprLit, ExprRange, ExprUnary, Fields, FnArg, GenericArgument, GenericParam,
    Item, ItemEnum, ItemFn, ItemStruct, Lit, LitStr, Pat, PathArguments, RangeLimits, ReturnType,
    Type, UnOp,
};

// The unit grammar, compiled from the same file forge-reflect uses at run time.
#[path = "../../src/units_core.rs"]
#[allow(dead_code)]
mod units_core;

/// See the `forge_reflect` crate docs for the attribute language.
#[proc_macro_attribute]
pub fn forge_api(attr: TokenStream, item: TokenStream) -> TokenStream {
    let out = parse_item_args(attr.into()).and_then(|args| {
        let item: Item = syn::parse(item)?;
        match item {
            Item::Fn(f) => expand_fn(&args, f),
            Item::Struct(s) => expand_struct(&args, s),
            Item::Enum(e) => expand_enum(&args, e),
            other => Err(syn::Error::new(
                other.span(),
                "#[forge_api] goes on a free fn, a struct with named fields, or an enum",
            )),
        }
    });
    out.unwrap_or_else(syn::Error::into_compile_error).into()
}

// ---- attribute language ----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Purity {
    Pure,
    Read,
    Mutate,
}

impl Purity {
    fn tokens(self) -> TokenStream2 {
        match self {
            Self::Pure => quote!(__fr::Purity::Pure),
            Self::Read => quote!(__fr::Purity::Read),
            Self::Mutate => quote!(__fr::Purity::Mutate),
        }
    }
}

#[derive(Default)]
struct ItemArgs {
    name: Option<String>,
    category: Option<String>,
    purity: Option<(Purity, Span)>,
    destructive: Option<Span>,
    returns: Option<(FieldArgs, Span)>,
}

/// `#[forge(...)]` on a field / parameter / variant, or `returns(...)`.
#[derive(Default, Clone)]
struct FieldArgs {
    name: Option<String>,
    doc: Option<(String, Span)>,
    category: Option<String>,
    units: Option<String>,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    read_only: bool,
    hidden: bool,
    widget: Option<String>,
    entity: bool,
}

fn parse_item_args(ts: TokenStream2) -> syn::Result<ItemArgs> {
    let mut a = ItemArgs::default();
    let parser = syn::meta::parser(|meta| {
        let p = &meta.path;
        if p.is_ident("name") {
            a.name = Some(meta.value()?.parse::<LitStr>()?.value());
        } else if p.is_ident("category") {
            a.category = Some(meta.value()?.parse::<LitStr>()?.value());
        } else if p.is_ident("pure") {
            a.purity = Some((Purity::Pure, p.span()));
        } else if p.is_ident("reads") {
            a.purity = Some((Purity::Read, p.span()));
        } else if p.is_ident("mutates") {
            a.purity = Some((Purity::Mutate, p.span()));
        } else if p.is_ident("destructive") {
            a.destructive = Some(p.span());
        } else if p.is_ident("returns") {
            let span = p.span();
            let mut f = FieldArgs::default();
            meta.parse_nested_meta(|m| parse_field_arg(&mut f, &m))?;
            check_range(&f, span)?;
            a.returns = Some((f, span));
        } else {
            return Err(meta.error(
                "unknown #[forge_api] argument; expected name, category, pure, reads, mutates, \
                 destructive, returns(...)",
            ));
        }
        Ok(())
    });
    syn::parse::Parser::parse2(parser, ts)?;
    Ok(a)
}

fn lit_f64(e: &Expr) -> syn::Result<f64> {
    match e {
        Expr::Lit(ExprLit {
            lit: Lit::Float(f), ..
        }) => f.base10_parse::<f64>(),
        Expr::Lit(ExprLit {
            lit: Lit::Int(i), ..
        }) => i.base10_parse::<f64>(),
        Expr::Unary(ExprUnary {
            op: UnOp::Neg(_),
            expr,
            ..
        }) => lit_f64(expr).map(|v| -v),
        Expr::Paren(p) => lit_f64(&p.expr),
        Expr::Group(g) => lit_f64(&g.expr),
        other => Err(syn::Error::new(other.span(), "expected a number literal")),
    }
}

fn parse_field_arg(f: &mut FieldArgs, meta: &ParseNestedMeta<'_>) -> syn::Result<()> {
    let p = &meta.path;
    let string = || -> syn::Result<LitStr> { meta.value()?.parse::<LitStr>() };
    if p.is_ident("name") {
        f.name = Some(string()?.value());
    } else if p.is_ident("doc") {
        let s = string()?;
        f.doc = Some((s.value(), s.span()));
    } else if p.is_ident("category") {
        f.category = Some(string()?.value());
    } else if p.is_ident("widget") {
        f.widget = Some(string()?.value());
    } else if p.is_ident("units") {
        let s = string()?;
        let v = s.value();
        units_core::parse_unit(&v)
            .map_err(|why| syn::Error::new(s.span(), format!("unit `{v}`: {why}")))?;
        f.units = Some(v);
    } else if p.is_ident("min") {
        f.min = Some(lit_f64(&meta.value()?.parse::<Expr>()?)?);
    } else if p.is_ident("max") {
        f.max = Some(lit_f64(&meta.value()?.parse::<Expr>()?)?);
    } else if p.is_ident("step") {
        f.step = Some(lit_f64(&meta.value()?.parse::<Expr>()?)?);
    } else if p.is_ident("range") {
        let e: Expr = meta.value()?.parse()?;
        let Expr::Range(ExprRange {
            start: Some(lo),
            end: Some(hi),
            limits: RangeLimits::Closed(_),
            ..
        }) = &e
        else {
            return Err(syn::Error::new(
                e.span(),
                "range is inclusive: `range = 0.0..=1.0`",
            ));
        };
        f.min = Some(lit_f64(lo)?);
        f.max = Some(lit_f64(hi)?);
    } else if p.is_ident("read_only") {
        f.read_only = true;
    } else if p.is_ident("hidden") {
        f.hidden = true;
    } else if p.is_ident("entity") {
        f.entity = true;
    } else {
        return Err(meta.error(
            "unknown #[forge] argument; expected name, doc, category, units, min, max, step, \
             range, read_only, hidden, widget, entity",
        ));
    }
    Ok(())
}

fn check_range(f: &FieldArgs, span: Span) -> syn::Result<()> {
    if let (Some(lo), Some(hi)) = (f.min, f.max)
        && lo > hi
    {
        return Err(syn::Error::new(
            span,
            format!("empty range: min {lo} > max {hi}"),
        ));
    }
    if f.step.is_some_and(|s| s <= 0.0) {
        return Err(syn::Error::new(span, "step must be positive"));
    }
    Ok(())
}

/// Pull `#[forge(...)]` off `attrs` (it is not a real attribute) and parse it.
fn take_forge_attrs(attrs: &mut Vec<Attribute>) -> syn::Result<(FieldArgs, Option<Span>)> {
    let mut f = FieldArgs::default();
    let mut span = None;
    let mut err: Option<syn::Error> = None;
    attrs.retain(|a| {
        if !a.path().is_ident("forge") {
            return true;
        }
        span = Some(a.span());
        if let Err(e) = a.parse_nested_meta(|m| parse_field_arg(&mut f, &m)) {
            match &mut err {
                Some(prev) => prev.combine(e),
                None => err = Some(e),
            }
        }
        false
    });
    if let Some(e) = err {
        return Err(e);
    }
    if let Some(s) = span {
        check_range(&f, s)?;
    }
    Ok((f, span))
}

/// Doc comment text: `#[doc = "..."]` lines, one leading space dropped each, joined, trimmed.
fn doc_of(attrs: &[Attribute]) -> String {
    attrs
        .iter()
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) if nv.path.is_ident("doc") => match &nv.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        })
        .map(|l| l.strip_prefix(' ').map(str::to_string).unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// `max_speed` → `Max Speed`, `ThrusterConfig` → `Thruster Config`.
fn humanize(ident: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for part in ident
        .trim_start_matches("r#")
        .split('_')
        .filter(|s| !s.is_empty())
    {
        let mut cur = String::new();
        let mut prev_lower = false;
        for c in part.chars() {
            if c.is_uppercase() && prev_lower && !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = c.is_lowercase() || c.is_ascii_digit();
            cur.push(c);
        }
        if !cur.is_empty() {
            words.push(cur);
        }
    }
    words
        .iter()
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn opt_str(s: Option<&str>) -> TokenStream2 {
    match s {
        Some(s) => quote!(::core::option::Option::Some(#s)),
        None => quote!(::core::option::Option::None),
    }
}

fn opt_f64(v: Option<f64>) -> TokenStream2 {
    match v {
        Some(v) => {
            let lit = Literal::f64_suffixed(v.abs());
            if v.is_sign_negative() {
                quote!(::core::option::Option::Some(-#lit))
            } else {
                quote!(::core::option::Option::Some(#lit))
            }
        }
        None => quote!(::core::option::Option::None),
    }
}

fn meta_tokens(f: &FieldArgs, ident: &str, doc: &str) -> TokenStream2 {
    let display = f.name.clone().unwrap_or_else(|| humanize(ident));
    let doc = f.doc.as_ref().map_or(doc.to_string(), |d| d.0.clone());
    let category = opt_str(f.category.as_deref());
    let units = opt_str(f.units.as_deref());
    let widget = opt_str(f.widget.as_deref());
    let (min, max, step) = (opt_f64(f.min), opt_f64(f.max), opt_f64(f.step));
    let (read_only, hidden, entity) = (f.read_only, f.hidden, f.entity);
    quote! {
        __fr::FieldMeta {
            display_name: #display,
            doc: #doc,
            category: #category,
            units: #units,
            min: #min,
            max: #max,
            step: #step,
            read_only: #read_only,
            hidden: #hidden,
            widget: #widget,
            entity: #entity,
        }
    }
}

/// A pin / field / property.
struct PinIn {
    name: String,
    ty: Type,
    args: FieldArgs,
    doc: String,
}

fn pin_tokens(p: &PinIn) -> TokenStream2 {
    let name = &p.name;
    let ty = &p.ty;
    let meta = meta_tokens(&p.args, &p.name, &p.doc);
    quote! {
        __fr::PinSpec { name: #name, ty: __fr::TypeDesc::of::<#ty>(), meta: #meta }
    }
}

fn require_doc(doc: &str, span: Span) -> syn::Result<()> {
    if doc.is_empty() {
        Err(syn::Error::new(
            span,
            "#[forge_api] items need a doc comment: it is the node tooltip and the API \
             description (Ch.6)",
        ))
    } else {
        Ok(())
    }
}

fn reject_generics(g: &syn::Generics) -> syn::Result<()> {
    match g
        .params
        .iter()
        .find(|p| !matches!(p, GenericParam::Lifetime(_)))
    {
        Some(p) => Err(syn::Error::new(
            p.span(),
            "#[forge_api] items cannot be generic yet: a node / schema needs one concrete type",
        )),
        None => Ok(()),
    }
}

/// Drop the last element — the `mutate-drift` positive-control mutant (W2). Only ever active
/// when forge-reflect's `mutate-drift` feature is on.
fn drift<T>(v: &mut Vec<T>) {
    if cfg!(feature = "mutate-drift") {
        v.pop();
    }
}

// ---- fn ----------------------------------------------------------------------------------

fn result_ok_type(ty: &Type) -> Option<&Type> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != "Result" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args.iter().find_map(|a| match a {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

fn is_unit(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(t) if t.elems.is_empty())
}

fn expand_fn(args: &ItemArgs, mut f: ItemFn) -> syn::Result<TokenStream2> {
    let ident = f.sig.ident.clone();
    let ident_s = ident.to_string();
    let doc = doc_of(&f.attrs);
    require_doc(&doc, ident.span())?;
    reject_generics(&f.sig.generics)?;
    if let Some(a) = &f.sig.asyncness {
        return Err(syn::Error::new(
            a.span(),
            "#[forge_api] fns are synchronous",
        ));
    }

    let mut context: Vec<(String, String, bool)> = Vec::new();
    let mut pins: Vec<PinIn> = Vec::new();
    for input in &mut f.sig.inputs {
        let pt = match input {
            FnArg::Receiver(r) => {
                return Err(syn::Error::new(
                    r.span(),
                    "#[forge_api] goes on a free fn (methods are not supported yet)",
                ));
            }
            FnArg::Typed(pt) => pt,
        };
        let (fa, fspan) = take_forge_attrs(&mut pt.attrs)?;
        let Pat::Ident(pi) = &*pt.pat else {
            return Err(syn::Error::new(
                pt.pat.span(),
                "parameters must be plain identifiers",
            ));
        };
        let name = pi.ident.to_string();
        match &*pt.ty {
            Type::Reference(r) => {
                if let Some(s) = fspan {
                    return Err(syn::Error::new(
                        s,
                        "a reference parameter is the execution context, not a pin: it takes no \
                         #[forge(...)]",
                    ));
                }
                let tn = r.elem.to_token_stream().to_string();
                context.push((name, tn, r.mutability.is_some()));
            }
            Type::ImplTrait(t) => {
                return Err(syn::Error::new(
                    t.span(),
                    "a pin needs a concrete type, not impl Trait",
                ));
            }
            ty => pins.push(PinIn {
                name,
                ty: ty.clone(),
                args: fa,
                doc: String::new(),
            }),
        }
    }

    let inferred = if context.iter().any(|c| c.2) {
        Purity::Mutate
    } else if context.is_empty() {
        Purity::Pure
    } else {
        Purity::Read
    };
    if let Some((p, span)) = args.purity
        && p != inferred
    {
        return Err(syn::Error::new(
            span,
            format!(
                "declared {p:?} but the signature says {inferred:?}: purity follows the context \
                 parameters (none = Pure, `&` = Read, `&mut` = Mutate)"
            ),
        ));
    }
    if let Some(s) = args.destructive
        && inferred != Purity::Mutate
    {
        return Err(syn::Error::new(
            s,
            "only a mutating fn (`&mut` context) can be destructive",
        ));
    }

    let (fallible, out_ty) = match &f.sig.output {
        ReturnType::Default => (false, None),
        ReturnType::Type(_, ty) => match result_ok_type(ty) {
            Some(t) => (true, (!is_unit(t)).then(|| t.clone())),
            None => (false, (!is_unit(ty)).then(|| (**ty).clone())),
        },
    };
    if let Some(t) = &out_ty {
        if matches!(t, Type::Tuple(_)) {
            return Err(syn::Error::new(
                t.span(),
                "several outputs: return a #[forge_api] struct instead of a tuple",
            ));
        }
        if matches!(t, Type::Reference(_)) {
            return Err(syn::Error::new(
                t.span(),
                "an output pin needs an owned value",
            ));
        }
    }
    let out_pin = match (&out_ty, &args.returns) {
        (Some(t), r) => Some(PinIn {
            name: "return".to_string(),
            ty: t.clone(),
            args: r.as_ref().map(|r| r.0.clone()).unwrap_or_default(),
            doc: String::new(),
        }),
        (None, Some((_, s))) => {
            return Err(syn::Error::new(*s, "returns(...) on a fn with no output"));
        }
        (None, None) => None,
    };

    let path = quote!(::core::concat!(::core::module_path!(), "::", #ident_s));
    let display = args.name.clone().unwrap_or_else(|| humanize(&ident_s));
    let category = opt_str(args.category.as_deref());
    let purity = inferred.tokens();
    let destructive = args.destructive.is_some();
    let ctx = context
        .iter()
        .map(|(n, t, m)| quote!(__fr::ContextParam { name: #n, type_name: #t, mutable: #m }));

    // Output 1: bevy_reflect, straight from the signature's types.
    let reflect_args = pins.iter().map(|p| {
        let (n, ty) = (&p.name, &p.ty);
        quote!(__fr::ReflectArg { name: #n, info: <#ty as __fr::Typed>::type_info() })
    });
    let reflect_ret = match &out_ty {
        Some(t) => quote!(::core::option::Option::Some(<#t as __fr::Typed>::type_info())),
        None => quote!(::core::option::Option::None),
    };
    // Output 2: node pins.
    let mut node_inputs: Vec<TokenStream2> = pins.iter().map(pin_tokens).collect();
    drift(&mut node_inputs);
    let node_output = match &out_pin {
        Some(p) => {
            let t = pin_tokens(p);
            quote!(::core::option::Option::Some(#t))
        }
        None => quote!(::core::option::Option::None),
    };
    // Output 3: schema properties.
    let schema_props: Vec<TokenStream2> = pins.iter().map(pin_tokens).collect();
    let schema_return = node_output.clone();
    // Output 4: command fields, only when mutating.
    let command_fields = if inferred == Purity::Mutate {
        let fields: Vec<TokenStream2> = pins.iter().map(pin_tokens).collect();
        quote!(::core::option::Option::Some(::std::vec![#(#fields),*]))
    } else {
        quote!(::core::option::Option::None)
    };

    let vis = f.vis.clone();
    let marker_doc = format!("`#[forge_api]` descriptor of the fn [`{ident_s}`].");
    Ok(quote! {
        #f

        #[doc = #marker_doc]
        #[doc(hidden)]
        #[allow(non_camel_case_types, dead_code)]
        #vis struct #ident {}

        impl ::forge_reflect::__private::ForgeApi for #ident {
            fn describe() -> ::forge_reflect::__private::ApiItem {
                use ::forge_reflect::__private as __fr;
                __fr::ApiItem::function(__fr::FnSpec {
                    path: #path,
                    ident: #ident_s,
                    display_name: #display,
                    category: #category,
                    doc: #doc,
                    purity: #purity,
                    destructive: #destructive,
                    fallible: #fallible,
                    context: ::std::vec![#(#ctx),*],
                    reflect_args: ::std::vec![#(#reflect_args),*],
                    reflect_ret: #reflect_ret,
                    node_inputs: ::std::vec![#(#node_inputs),*],
                    node_output: #node_output,
                    schema_props: ::std::vec![#(#schema_props),*],
                    schema_return: #schema_return,
                    command_fields: #command_fields,
                })
            }
        }
    })
}

// ---- struct / enum -----------------------------------------------------------------------

fn named_fields(fields: &mut Fields, what: &str) -> syn::Result<Vec<PinIn>> {
    let named = match fields {
        Fields::Named(n) => n,
        Fields::Unit => return Ok(Vec::new()),
        Fields::Unnamed(u) => {
            return Err(syn::Error::new(
                u.span(),
                format!("{what}: tuple fields are not supported yet — name the fields"),
            ));
        }
    };
    let mut out = Vec::new();
    for field in &mut named.named {
        let (fa, _) = take_forge_attrs(&mut field.attrs)?;
        if let Some((_, s)) = &fa.doc {
            return Err(syn::Error::new(
                *s,
                "a field's tooltip is its doc comment, not `doc = ...`",
            ));
        }
        for a in &field.attrs {
            if a.path().is_ident("reflect") {
                let text = a.to_token_stream().to_string();
                if text.contains("ignore") || text.contains("skip") {
                    return Err(syn::Error::new(
                        a.span(),
                        "#[forge_api] fields are all reflected: an ignored field would drift the \
                         outputs",
                    ));
                }
            }
        }
        let name = field
            .ident
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        out.push(PinIn {
            name,
            ty: field.ty.clone(),
            doc: doc_of(&field.attrs),
            args: fa,
        });
    }
    Ok(out)
}

/// The glob that puts `bevy_reflect` in scope for its derive (see `forge_reflect::__private`).
fn reflect_prelude() -> TokenStream2 {
    quote! {
        #[allow(unused_imports)]
        use ::forge_reflect::__private::reexport::*;
    }
}

fn expand_struct(args: &ItemArgs, mut s: ItemStruct) -> syn::Result<TokenStream2> {
    let ident = s.ident.clone();
    let ident_s = ident.to_string();
    let doc = doc_of(&s.attrs);
    require_doc(&doc, ident.span())?;
    reject_generics(&s.generics)?;
    if args.purity.is_some() || args.destructive.is_some() || args.returns.is_some() {
        return Err(syn::Error::new(
            ident.span(),
            "pure / reads / mutates / destructive / returns apply to fns",
        ));
    }
    if matches!(s.fields, Fields::Unit) {
        return Err(syn::Error::new(
            ident.span(),
            "a #[forge_api] struct has named fields",
        ));
    }
    let fields = named_fields(&mut s.fields, "struct")?;
    let display = args.name.clone().unwrap_or_else(|| humanize(&ident_s));
    let category = opt_str(args.category.as_deref());
    let mut node_fields: Vec<TokenStream2> = fields.iter().map(pin_tokens).collect();
    drift(&mut node_fields);
    let schema_fields: Vec<TokenStream2> = fields.iter().map(pin_tokens).collect();
    let prelude = reflect_prelude();
    Ok(quote! {
        #prelude
        #[derive(::forge_reflect::__private::Reflect)]
        #s

        const _: () = {
            use ::forge_reflect::__private as __fr;
            fn __forge_spec() -> __fr::StructSpec {
                __fr::StructSpec {
                    ty: __fr::TypeDesc::of::<#ident>(),
                    info: <#ident as __fr::Typed>::type_info(),
                    ident: #ident_s,
                    display_name: #display,
                    category: #category,
                    doc: #doc,
                    node_fields: ::std::vec![#(#node_fields),*],
                    schema_fields: ::std::vec![#(#schema_fields),*],
                }
            }
            impl __fr::ForgeType for #ident {
                fn pin_kind() -> __fr::PinKind {
                    __fr::PinKind::Struct
                }
                fn json_schema(defs: &mut __fr::SchemaDefs) -> __fr::Value {
                    defs.define(<#ident as __fr::TypePath>::type_path(), |defs| {
                        __fr::struct_schema(&__forge_spec(), defs)
                    })
                }
            }
            impl __fr::ForgeApi for #ident {
                fn describe() -> __fr::ApiItem {
                    __fr::ApiItem::structure(__forge_spec())
                }
            }
        };
    })
}

fn expand_enum(args: &ItemArgs, mut e: ItemEnum) -> syn::Result<TokenStream2> {
    let ident = e.ident.clone();
    let ident_s = ident.to_string();
    let doc = doc_of(&e.attrs);
    require_doc(&doc, ident.span())?;
    reject_generics(&e.generics)?;
    if args.purity.is_some() || args.destructive.is_some() || args.returns.is_some() {
        return Err(syn::Error::new(
            ident.span(),
            "pure / reads / mutates / destructive / returns apply to fns",
        ));
    }
    if e.variants.is_empty() {
        return Err(syn::Error::new(
            ident.span(),
            "a #[forge_api] enum needs a variant",
        ));
    }
    let mut variants = Vec::new();
    for v in &mut e.variants {
        let (va, _) = take_forge_attrs(&mut v.attrs)?;
        let only_name = va.doc.is_none()
            && va.category.is_none()
            && va.units.is_none()
            && va.widget.is_none()
            && va.min.is_none()
            && va.max.is_none()
            && va.step.is_none()
            && !(va.read_only || va.hidden || va.entity);
        if !only_name {
            return Err(syn::Error::new(
                v.ident.span(),
                "a variant takes only #[forge(name = \"...\")]",
            ));
        }
        let vname = v.ident.to_string();
        let vdisplay = va.name.clone().unwrap_or_else(|| humanize(&vname));
        let vdoc = doc_of(&v.attrs);
        let fields = named_fields(&mut v.fields, "enum variant")?;
        variants.push((vname, vdisplay, vdoc, fields));
    }
    let variant_tokens = |vs: &[(String, String, String, Vec<PinIn>)]| -> Vec<TokenStream2> {
        vs.iter()
            .map(|(n, d, doc, fields)| {
                let f: Vec<TokenStream2> = fields.iter().map(pin_tokens).collect();
                quote!(__fr::VariantSpec {
                    name: #n, display_name: #d, doc: #doc, fields: ::std::vec![#(#f),*]
                })
            })
            .collect()
    };
    let mut node_variants = variant_tokens(&variants);
    drift(&mut node_variants);
    let schema_variants = variant_tokens(&variants);
    let display = args.name.clone().unwrap_or_else(|| humanize(&ident_s));
    let category = opt_str(args.category.as_deref());
    let prelude = reflect_prelude();
    Ok(quote! {
        #prelude
        #[derive(::forge_reflect::__private::Reflect)]
        #e

        const _: () = {
            use ::forge_reflect::__private as __fr;
            fn __forge_spec() -> __fr::EnumSpec {
                __fr::EnumSpec {
                    ty: __fr::TypeDesc::of::<#ident>(),
                    info: <#ident as __fr::Typed>::type_info(),
                    ident: #ident_s,
                    display_name: #display,
                    category: #category,
                    doc: #doc,
                    node_variants: ::std::vec![#(#node_variants),*],
                    schema_variants: ::std::vec![#(#schema_variants),*],
                }
            }
            impl __fr::ForgeType for #ident {
                fn pin_kind() -> __fr::PinKind {
                    __fr::PinKind::Enum
                }
                fn json_schema(defs: &mut __fr::SchemaDefs) -> __fr::Value {
                    defs.define(<#ident as __fr::TypePath>::type_path(), |defs| {
                        __fr::enum_schema(&__forge_spec(), defs)
                    })
                }
            }
            impl __fr::ForgeApi for #ident {
                fn describe() -> __fr::ApiItem {
                    __fr::ApiItem::enumeration(__forge_spec())
                }
            }
        };
    })
}
