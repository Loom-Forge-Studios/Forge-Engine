//! `test_shader_hot_reload` — WGSL hot reload keeps the last good shader (Ch.9, M1-1).
//!
//! For each store backend (MemoryStore; LocalFs edited by a plain `std::fs::write`, as a
//! text editor would):
//!
//! 1. load v1 and run it on the GPU: the output is 1;
//! 2. **good swap:** save v2, poll → `Reloaded` (generation 2); the rebuilt pipeline outputs 2;
//! 3. **bad shader:** save a syntax error, then a validation error, then delete the file —
//!    each poll → `Failed` naming `path:line:col` (or the deletion), and the module in
//!    service still outputs 2;
//! 4. fix it (v3) → `Reloaded` (generation 3), output 3; a quiet poll reports nothing.
//!
//! Positive controls (W2), `positive_control_*`: the broken shaders really are rejected by
//! wgpu itself when created directly (so "kept last good" is not vacuous), and v1 and v2
//! really produce different output (so "swapped" is observable).

use std::path::PathBuf;

use forge_gpu::shader::{self, validate_wgsl};
use forge_gpu::wgpu;
use forge_gpu::{
    AdapterPool, GpuDevice, PoolOptions, ShaderEvent, ShaderId, ShaderLibrary, transfer,
};
use forge_store::{Bytes, LocalFs, MemoryStore, ProjectStore, StorePath};

fn good(n: u32) -> String {
    format!(
        "@group(0) @binding(0) var<storage, read_write> o: array<u32>;\n\
         @compute @workgroup_size(1)\n\
         fn main() {{ o[0] = {n}u; }}\n"
    )
}

/// Line 3 has a syntax error (missing `;` and a stray token).
const SYNTAX_ERROR: &str = "@group(0) @binding(0) var<storage, read_write> o: array<u32>;\n\
@compute @workgroup_size(1)\n\
fn main() { o[0] = 9u oops }\n";

/// Parses, but fails validation: a float stored into a u32 array.
const TYPE_ERROR: &str = "@group(0) @binding(0) var<storage, read_write> o: array<u32>;\n\
@compute @workgroup_size(1)\n\
fn main() { o[0] = 1.5; }\n";

mod common;

fn pool() -> Option<AdapterPool> {
    common::pool(&PoolOptions::default())
}

/// Run `module`'s `main` once and return `o[0]`.
fn run(dev: &GpuDevice, module: &wgpu::ShaderModule) -> u32 {
    let pipeline = dev
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("probe"),
            layout: None,
            module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let buf = transfer::upload_buffer(dev, "o", &[0u8; 16], wgpu::BufferUsages::STORAGE);
    let bg = dev.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    });
    let mut enc = dev.device.create_command_encoder(&Default::default());
    {
        let mut cp = enc.begin_compute_pass(&Default::default());
        cp.set_pipeline(&pipeline);
        cp.set_bind_group(0, &bg, &[]);
        cp.dispatch_workgroups(1, 1, 1);
    }
    dev.queue.submit([enc.finish()]);
    let b = transfer::read_buffer(dev, &buf, 0, 4).expect("readback");
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Where edits land: through the store (MemoryStore) or straight on disk (LocalFs).
enum Editor {
    Memory(MemoryStore),
    Disk(LocalFs, PathBuf),
}

impl Editor {
    fn store(&self) -> &dyn ProjectStore {
        match self {
            Editor::Memory(s) => s,
            Editor::Disk(s, _) => s,
        }
    }

    fn save(&mut self, path: &StorePath, text: &str) {
        match self {
            Editor::Memory(s) => s.write(path, Bytes::from(text.to_string())).expect("write"),
            Editor::Disk(_, root) => {
                let p = root.join(path.as_str());
                std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
                std::fs::write(p, text).expect("write");
            }
        }
    }

    fn delete(&mut self, path: &StorePath) {
        match self {
            Editor::Memory(s) => s.delete(path).expect("delete"),
            Editor::Disk(_, root) => std::fs::remove_file(root.join(path.as_str())).expect("rm"),
        }
    }
}

fn one(events: Vec<ShaderEvent>) -> ShaderEvent {
    assert_eq!(events.len(), 1, "{events:?}");
    events.into_iter().next().expect("one event")
}

fn scenario(dev: &GpuDevice, mut ed: Editor) {
    let path = StorePath::new("shaders/probe.wgsl").expect("path");
    ed.save(&path, &good(1));
    let mut lib = ShaderLibrary::new();
    let id: ShaderId = lib.load(ed.store(), &dev.device, &path).expect("v1 loads");
    assert_eq!(lib.generation(id), 1);
    assert_eq!(run(dev, lib.module(id).expect("module")), 1);
    assert!(
        lib.poll(ed.store(), &dev.device).expect("poll").is_empty(),
        "quiet"
    );

    // Good swap.
    ed.save(&path, &good(2));
    match one(lib.poll(ed.store(), &dev.device).expect("poll")) {
        ShaderEvent::Reloaded {
            generation,
            path: p,
            ..
        } => {
            assert_eq!(generation, 2);
            assert_eq!(p, "shaders/probe.wgsl");
        }
        e => panic!("expected a reload, got {e:?}"),
    }
    assert_eq!(run(dev, lib.module(id).expect("module")), 2);

    // Bad shaders keep the last good one.
    for (text, needle) in [
        (SYNTAX_ERROR, "shaders/probe.wgsl:3:"),
        (TYPE_ERROR, "shaders/probe.wgsl:3:"),
    ] {
        ed.save(&path, text);
        match one(lib.poll(ed.store(), &dev.device).expect("poll")) {
            ShaderEvent::Failed { error, .. } => {
                let s = error.to_string();
                assert!(s.starts_with("GPU-0004"), "{s}");
                assert!(s.contains(needle), "wanted {needle:?} in:\n{s}");
            }
            e => panic!("expected a failure, got {e:?}"),
        }
        assert_eq!(lib.generation(id), 2, "generation unchanged");
        assert!(lib.last_error(id).is_some());
        assert_eq!(
            run(dev, lib.module(id).expect("module")),
            2,
            "last good in service"
        );
        assert!(
            lib.poll(ed.store(), &dev.device).expect("poll").is_empty(),
            "reported once"
        );
    }
    ed.delete(&path);
    match one(lib.poll(ed.store(), &dev.device).expect("poll")) {
        ShaderEvent::Failed { error, .. } => {
            assert!(error.to_string().starts_with("GPU-0005"), "{error}");
        }
        e => panic!("expected a failure, got {e:?}"),
    }
    assert_eq!(
        run(dev, lib.module(id).expect("module")),
        2,
        "last good survives deletion"
    );

    // Fixed.
    ed.save(&path, &good(3));
    match one(lib.poll(ed.store(), &dev.device).expect("poll")) {
        ShaderEvent::Reloaded { generation, .. } => assert_eq!(generation, 3),
        e => panic!("expected a reload, got {e:?}"),
    }
    assert!(lib.last_error(id).is_none());
    assert_eq!(run(dev, lib.module(id).expect("module")), 3);
    assert!(lib.poll(ed.store(), &dev.device).expect("poll").is_empty());
    assert!(
        dev.take_uncaptured_errors().is_empty(),
        "no error escaped to wgpu's handler"
    );
}

#[test]
fn hot_reload_on_memory_store() {
    let Some(pool) = pool() else { return };
    scenario(pool.primary(), Editor::Memory(MemoryStore::new("ada")));
}

#[test]
fn hot_reload_on_a_real_folder_edited_outside_the_engine() {
    let Some(pool) = pool() else { return };
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shader-hot-reload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = LocalFs::open(&root, "ada").expect("open");
    scenario(pool.primary(), Editor::Disk(store, root.clone()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_invalid_shader_at_load_is_an_error_not_a_panic() {
    let Some(pool) = pool() else { return };
    let dev = pool.primary();
    let mut store = MemoryStore::new("ada");
    let path = StorePath::new("bad.wgsl").expect("path");
    store
        .write(&path, Bytes::from_static(SYNTAX_ERROR.as_bytes()))
        .expect("write");
    let mut lib = ShaderLibrary::new();
    let e = lib.load(&store, &dev.device, &path).expect_err("invalid");
    assert!(e.to_string().contains("bad.wgsl:3:"), "{e}");
    assert!(lib.is_empty());
    let missing = StorePath::new("missing.wgsl").expect("path");
    assert!(lib.load(&store, &dev.device, &missing).is_err());
}

#[test]
fn positive_control_the_broken_shaders_are_really_broken() {
    // naga refuses both...
    assert!(validate_wgsl("s.wgsl", SYNTAX_ERROR).is_err());
    assert!(validate_wgsl("t.wgsl", TYPE_ERROR).is_err());
    assert!(validate_wgsl("g.wgsl", &good(1)).is_ok());
    // ...and so does wgpu when handed the text directly, bypassing the library: serving
    // either would break the pipeline, so "kept the last good module" is a real property.
    let Some(pool) = pool() else { return };
    let dev = pool.primary();
    for (name, text) in [("syntax", SYNTAX_ERROR), ("type", TYPE_ERROR)] {
        let scope = dev.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _m = dev
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(text.into()),
            });
        assert!(
            pollster::block_on(scope.pop()).is_some(),
            "wgpu accepted the {name} shader"
        );
    }
}

#[test]
fn positive_control_v1_and_v2_are_distinguishable() {
    let Some(pool) = pool() else { return };
    let dev = pool.primary();
    let a = shader::compile_wgsl(&dev.device, "a", &good(1)).expect("v1");
    let b = shader::compile_wgsl(&dev.device, "b", &good(2)).expect("v2");
    assert_ne!(run(dev, &a), run(dev, &b));
}
