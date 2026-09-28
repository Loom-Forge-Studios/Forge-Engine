//! The `Importer` adapter (WP-21): a WASM plugin imports a file format.
//!
//! `forge-asset` defines the point; hosts register this adapter ([`adapt_importers`]) so a
//! plugin that declares `provides: [Importer("csvmesh")]` is an ordinary importer in the
//! asset database — sidecars, content-addressed artefacts, dependency tracking and reimport
//! work exactly as for the first-party glTF and image importers (I16).
//!
//! ## The protocol
//!
//! **Describe** (once, at install): `call("Importer", key, [])` returns JSON
//!
//! ```json
//! { "version": 1, "extensions": ["csvmesh"] }
//! ```
//!
//! **Import**: the input is binary (bytes are never escaped into JSON):
//!
//! ```text
//! u32 LE header length | header JSON | u32 LE source length | source bytes
//!                      | then per supplied dependency: u32 LE length | bytes
//! header: { "path": "meshes/a.csvmesh", "settings": {"k": "v"}, "deps": ["b.bin"] }
//! ```
//!
//! and the output is
//!
//! ```text
//! u32 LE header length | header JSON | per artefact: u32 LE length | bytes
//! header: { "artefacts": [{"label": "", "kind": "mesh", "name": "A"}], "needs": [] }
//! ```
//!
//! An importer that needs another file names it in `needs` (a reference relative to the
//! source, resolved exactly by the VFS, M0-17): the host reads it through
//! `ImportCx::dependency` — recording it, so a change to it reimports — and calls again with
//! it supplied (`deps` lists them in order). Reading a file beyond the source needs
//! `Fs(ProjectRead)`, checked against the shared grant table at that moment. An empty `name`
//! is the source's file stem.
//!
//! **Pure and versioned.** An import is a function of the source, the settings and the
//! dependencies: the guest has no clock, files or network. Its version is the declared
//! `version` mixed with a stable hash of the plugin's code, so a hot reload (or new code at
//! the next start) reimports what it imported, and unchanged code never does.

use std::sync::Arc;

use bytes::Bytes;
use forge_asset::{AssetError, ImportCx, Importer, ImporterPoint};
use forge_plugin::{Capability, FsScope};
use serde::{Deserialize, Serialize};

use crate::plugin::fnv1a;
use crate::{GuestItem, WasmError, WasmHost};

/// Dependency rounds an import may take (each round supplies what the last one named).
pub const MAX_ROUNDS: usize = 8;
/// Dependencies one import may read.
pub const MAX_DEPS: usize = 64;

/// What a WASM importer says about itself at install.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Describe {
    /// Bumped by the author when the output for the same input changes.
    pub version: u32,
    /// The lower-case file extensions it claims.
    pub extensions: Vec<String>,
}

/// The import request's header.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputHeader {
    /// The source's project path.
    pub path: String,
    /// The import settings.
    pub settings: std::collections::BTreeMap<String, String>,
    /// The dependencies supplied after the source, in order.
    pub deps: Vec<String>,
}

/// One artefact in the import's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutArtefact {
    /// `""` for the main artefact.
    pub label: String,
    /// An asset type key (`mesh`, `texture`, `material`, `scene`).
    pub kind: String,
    /// Display name (`""`: the source's file stem).
    #[serde(default)]
    pub name: String,
}

/// The import answer's header.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputHeader {
    /// The artefacts, in the order their bytes follow.
    #[serde(default)]
    pub artefacts: Vec<OutArtefact>,
    /// Files (relative to the source) it needs before it can answer.
    #[serde(default)]
    pub needs: Vec<String>,
}

fn put_block(out: &mut Vec<u8>, b: &[u8]) -> Result<(), String> {
    let n = u32::try_from(b.len()).map_err(|_| format!("a block of {} bytes", b.len()))?;
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(b);
    Ok(())
}

fn take_block<'a>(input: &mut &'a [u8], what: &str) -> Result<&'a [u8], String> {
    let (len, rest) = input
        .split_first_chunk::<4>()
        .ok_or_else(|| format!("{what}: no length"))?;
    let n = u32::from_le_bytes(*len) as usize;
    if rest.len() < n {
        return Err(format!("{what}: {n} bytes declared, {} left", rest.len()));
    }
    let (b, rest) = rest.split_at(n);
    *input = rest;
    Ok(b)
}

/// Encode an import request (hosts; guest authors' tests).
pub fn encode_input(h: &InputHeader, source: &[u8], deps: &[&[u8]]) -> Result<Vec<u8>, String> {
    let head = serde_json::to_vec(h).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(head.len() + source.len() + 8);
    put_block(&mut out, &head)?;
    put_block(&mut out, source)?;
    for d in deps {
        put_block(&mut out, d)?;
    }
    Ok(out)
}

/// An import request decoded: its header, the source and the supplied dependencies.
pub type DecodedInput = (InputHeader, Vec<u8>, Vec<Vec<u8>>);

/// Decode an import request (what a guest reads; Rust-authored guests and tests).
pub fn decode_input(mut input: &[u8]) -> Result<DecodedInput, String> {
    let h: InputHeader = serde_json::from_slice(take_block(&mut input, "header")?)
        .map_err(|e| format!("header: {e}"))?;
    let source = take_block(&mut input, "source")?.to_vec();
    let mut deps = Vec::with_capacity(h.deps.len());
    for d in &h.deps {
        deps.push(take_block(&mut input, d)?.to_vec());
    }
    Ok((h, source, deps))
}

/// Encode an import answer (what a guest returns).
pub fn encode_output(h: &OutputHeader, artefacts: &[&[u8]]) -> Result<Vec<u8>, String> {
    let head = serde_json::to_vec(h).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    put_block(&mut out, &head)?;
    for a in artefacts {
        put_block(&mut out, a)?;
    }
    Ok(out)
}

/// Decode an import answer (the host).
pub fn decode_output(mut out: &[u8]) -> Result<(OutputHeader, Vec<Bytes>), String> {
    let h: OutputHeader = serde_json::from_slice(take_block(&mut out, "header")?)
        .map_err(|e| format!("header: {e}"))?;
    let mut bytes = Vec::with_capacity(h.artefacts.len());
    for a in &h.artefacts {
        bytes.push(Bytes::copy_from_slice(take_block(
            &mut out,
            &format!("artefact {:?}", a.label),
        )?));
    }
    if !out.is_empty() {
        return Err(format!("{} bytes after the last artefact", out.len()));
    }
    Ok((h, bytes))
}

/// Let WASM plugins provide and replace importers through `host` (the `Importer` point is
/// `forge-asset`'s; a host that runs an asset database registers this).
pub fn adapt_importers(host: &mut WasmHost) {
    host.adapt::<ImporterPoint>(|item| {
        let d = describe(&item)?;
        Ok(Arc::new(WasmImporter::new(item, d)) as Arc<dyn Importer>)
    });
}

fn describe(item: &GuestItem) -> Result<Describe, WasmError> {
    let out = item.call(&[])?;
    let d: Describe = serde_json::from_slice(&out).map_err(|e| WasmError::BadOutput {
        plugin: item.plugin().to_string(),
        item: item.name(),
        why: format!("its description: {e}"),
    })?;
    if d.extensions.is_empty()
        || d.extensions
            .iter()
            .any(|e| e.is_empty() || e.len() > 32 || e.contains(['.', '/', '\\']))
    {
        return Err(WasmError::BadOutput {
            plugin: item.plugin().to_string(),
            item: item.name(),
            why: format!(
                "extensions {:?}: one or more bare extensions (no dots or slashes)",
                d.extensions
            ),
        });
    }
    Ok(d)
}

/// A guest importer (see the module docs).
struct WasmImporter {
    item: GuestItem,
    declared: u32,
    /// The claimed extensions, lower-case. `Importer::extensions` lends `&[&str]` for the
    /// importer's lifetime; an importer is built once per install, so the few bytes are
    /// leaked once per install rather than kept in a self-referential struct.
    exts: Vec<&'static str>,
}

impl WasmImporter {
    fn new(item: GuestItem, d: Describe) -> Self {
        let exts = d
            .extensions
            .into_iter()
            .map(|e| &*Box::leak(e.to_ascii_lowercase().into_boxed_str()))
            .collect();
        Self {
            item,
            declared: d.version,
            exts,
        }
    }
}

impl Importer for WasmImporter {
    fn version(&self) -> u32 {
        let mut b = self.declared.to_le_bytes().to_vec();
        b.extend_from_slice(&self.item.code_hash().to_le_bytes());
        let h = fnv1a(&b);
        // Folded to 32 bits; never 0 (a sidecar's "no version").
        ((h >> 32) as u32 ^ h as u32).max(1)
    }

    fn extensions(&self) -> &[&str] {
        &self.exts
    }

    fn import(&self, cx: &mut ImportCx<'_>) -> Result<(), AssetError> {
        let path = cx.path().to_string();
        let settings = cx.settings().clone();
        let source = cx.source().clone();
        let mut deps: Vec<(String, Bytes)> = Vec::new();
        for _ in 0..MAX_ROUNDS {
            let header = InputHeader {
                path: path.clone(),
                settings: settings.clone(),
                deps: deps.iter().map(|(r, _)| r.clone()).collect(),
            };
            let dep_bytes: Vec<&[u8]> = deps.iter().map(|(_, b)| b.as_ref()).collect();
            let input = encode_input(&header, &source, &dep_bytes).map_err(|e| cx.fail(e))?;
            let out = self.item.call(&input).map_err(|e| cx.fail(e))?;
            let (h, bytes) = decode_output(&out).map_err(|e| {
                cx.fail(WasmError::BadOutput {
                    plugin: self.item.plugin().to_string(),
                    item: self.item.name(),
                    why: e,
                })
            })?;
            let missing: Vec<String> = h
                .needs
                .iter()
                .filter(|r| !deps.iter().any(|(d, _)| d == *r))
                .cloned()
                .collect();
            if missing.is_empty() {
                let stem = cx
                    .path()
                    .file_name()
                    .rsplit_once('.')
                    .map_or_else(|| cx.path().file_name().to_string(), |(s, _)| s.to_string());
                for (a, b) in h.artefacts.into_iter().zip(bytes) {
                    let name = if a.name.is_empty() {
                        stem.clone()
                    } else {
                        a.name
                    };
                    cx.emit(&a.label, &a.kind, &name, b);
                }
                return Ok(());
            }
            if deps.len() + missing.len() > MAX_DEPS {
                return Err(cx.fail(format!(
                    "{} asked for more than {MAX_DEPS} other files",
                    self.item.name()
                )));
            }
            // Reading beyond the source is reading the project (E-27): granted by a person.
            self.item
                .check(Capability::Fs(FsScope::ProjectRead))
                .map_err(|e| {
                    cx.fail(format!(
                        "{} needs {} to read {missing:?}: {e}",
                        self.item.plugin(),
                        Capability::Fs(FsScope::ProjectRead)
                    ))
                })?;
            for r in missing {
                let b = cx.dependency(&r)?;
                deps.push((r, b));
            }
        }
        Err(cx.fail(format!(
            "{} was still asking for files after {MAX_ROUNDS} rounds",
            self.item.name()
        )))
    }
}
