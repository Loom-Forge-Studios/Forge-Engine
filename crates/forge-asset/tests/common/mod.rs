//! Shared helpers for the forge-asset integration tests.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

use forge_asset::fixture::{self, SceneSpec};
use forge_asset::{AssetEvent, AssetServer, Settings};
use forge_store::{Bytes, LocalFs, MemoryStore, ProjectStore, StorePath};

/// Generous: loads take milliseconds; a timeout means a lost wake-up, not a slow machine.
pub const WAIT: Duration = Duration::from_secs(20);

pub fn p(s: &str) -> StorePath {
    StorePath::new(s).expect("path")
}

/// A fresh, empty directory under the target dir (never the repo tree).
pub fn tmp_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let d = base.join(format!("forge-asset-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("tmp dir");
    d
}

pub enum Backend {
    Memory,
    Local(&'static str),
}

pub fn store(b: &Backend) -> Box<dyn ProjectStore> {
    match b {
        Backend::Memory => Box::new(MemoryStore::new("ada")),
        Backend::Local(name) => Box::new(LocalFs::open(tmp_dir(name), "ada").expect("local fs")),
    }
}

/// Write a generated scene under `dir/` into `s`.
pub fn put_scene(s: &mut dyn ProjectStore, dir: &str, spec: &SceneSpec) {
    for (name, bytes) in fixture::scene(spec) {
        s.write(&p(&format!("{dir}/{name}")), bytes).expect("write");
    }
}

pub fn open(s: Box<dyn ProjectStore>) -> AssetServer {
    let x = forge_importers::asset_extensions().expect("extensions");
    AssetServer::open(s, &x).expect("opens").0
}

pub fn import(a: &mut AssetServer, path: &str) -> forge_asset::AssetId {
    let mut ev = Vec::new();
    a.import(&p(path), Settings::new(), &mut ev)
        .unwrap_or_else(|e| panic!("import {path}: {e}"))
}

pub fn write(a: &AssetServer, path: &str, b: impl Into<Bytes>) {
    a.vfs().write(&p(path), b.into()).expect("write");
}

pub fn reloaded(ev: &[AssetEvent]) -> Vec<forge_asset::AssetId> {
    ev.iter()
        .filter_map(|e| match e {
            AssetEvent::Reloaded { id } => Some(*id),
            _ => None,
        })
        .collect()
}
