//! WGSL-only shaders through naga, with hot reload (Ch.9).
//!
//! * **WGSL only.** Every shader is parsed and validated by naga here; the validated IR is
//!   handed to wgpu (`ShaderSource::Naga`), so the text is parsed once. forge-gpu enables no
//!   GLSL or SPIR-V front end (`test_wgsl_only`).
//! * **Hot reload.** [`ShaderLibrary::poll`] asks the project store for the stamps of just
//!   the shader files it holds (one `stamps_of` call: a handful of metadata reads on
//!   `LocalFs`), re-validates the changed ones, and swaps the module. A shader that fails —
//!   a syntax error, a validation error, a half-saved file, a deleted file — is reported
//!   with file, line and column, and **the last good module stays in service**: a typo
//!   never blanks the viewport.
//! * Consumers rebuild their pipelines when [`ShaderLibrary::generation`] moves.

use std::borrow::Cow;

use forge_store::{ProjectStore, Stamp, StorePath};

use crate::error::GpuError;

pub use wgpu::naga;

/// Parse and validate WGSL. The error names `path:line:col` on its first line, followed by
/// naga's annotated source excerpt.
pub fn validate_wgsl(path: &str, source: &str) -> Result<naga::Module, GpuError> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| {
        let at = e.location(source).map_or(String::new(), |l| {
            format!("{path}:{}:{}: ", l.line_number, l.line_position)
        });
        GpuError::Shader {
            path: path.to_string(),
            message: format!(
                "{at}{}\n{}",
                e.message(),
                e.emit_to_string_with_path(source, path)
            ),
        }
    })?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| {
        let at = e.location(source).map_or(String::new(), |l| {
            format!("{path}:{}:{}: ", l.line_number, l.line_position)
        });
        GpuError::Shader {
            path: path.to_string(),
            message: format!("{at}{}\n{}", e, e.emit_to_string_with_path(source, path)),
        }
    })?;
    Ok(module)
}

/// Create a wgpu module from validated IR, catching any error wgpu still raises (a
/// capability the device lacks) instead of letting it reach the uncaptured-error handler.
pub fn create_module(
    device: &wgpu::Device,
    label: &str,
    module: naga::Module,
) -> Result<wgpu::ShaderModule, GpuError> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let m = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
    });
    match pollster::block_on(scope.pop()) {
        None => Ok(m),
        Some(e) => Err(GpuError::Shader {
            path: label.to_string(),
            message: e.to_string(),
        }),
    }
}

/// Validate WGSL and create the module in one step (engine-internal shaders).
pub fn compile_wgsl(
    device: &wgpu::Device,
    path: &str,
    source: &str,
) -> Result<wgpu::ShaderModule, GpuError> {
    create_module(device, path, validate_wgsl(path, source)?)
}

/// A shader held by a [`ShaderLibrary`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShaderId(u32);

/// What a poll did to one shader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShaderEvent {
    /// A new version validated and is now in service.
    Reloaded {
        /// The shader.
        id: ShaderId,
        /// Its file.
        path: String,
        /// The new generation.
        generation: u64,
    },
    /// The file changed (or vanished) but the new version failed; the last good module
    /// stays in service.
    Failed {
        /// The shader.
        id: ShaderId,
        /// Its file.
        path: String,
        /// Why (with line and column).
        error: GpuError,
    },
}

struct Entry {
    path: StorePath,
    stamp: Option<Stamp>,
    module: wgpu::ShaderModule,
    generation: u64,
    error: Option<GpuError>,
}

/// The project's shaders, watched for change.
#[derive(Default)]
pub struct ShaderLibrary {
    entries: Vec<Entry>,
}

fn read_source(store: &dyn ProjectStore, path: &StorePath) -> Result<String, GpuError> {
    let bytes = store.read(path).map_err(|e| GpuError::ShaderIo {
        path: path.as_str().to_string(),
        why: e.to_string(),
    })?;
    String::from_utf8(bytes.to_vec()).map_err(|e| GpuError::ShaderIo {
        path: path.as_str().to_string(),
        why: format!("not UTF-8: {e}"),
    })
}

fn stamp_of(store: &dyn ProjectStore, path: &StorePath) -> Result<Option<Stamp>, GpuError> {
    let s = store
        .stamps_of(std::slice::from_ref(path))
        .map_err(|e| GpuError::ShaderIo {
            path: path.as_str().to_string(),
            why: e.to_string(),
        })?;
    Ok(s.into_iter().find(|(p, _)| p == path).map(|(_, s)| s))
}

impl ShaderLibrary {
    /// An empty library.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load `path` from the store, validate it and create its module. A shader that is
    /// invalid at load is an error (there is no last good version to serve yet).
    pub fn load(
        &mut self,
        store: &dyn ProjectStore,
        device: &wgpu::Device,
        path: &StorePath,
    ) -> Result<ShaderId, GpuError> {
        if let Some(i) = self.entries.iter().position(|e| e.path == *path) {
            return Ok(ShaderId(i as u32));
        }
        // Stamp before reading: an edit landing between the two is seen by the next poll.
        let stamp = stamp_of(store, path)?;
        let src = read_source(store, path)?;
        let module = compile_wgsl(device, path.as_str(), &src)?;
        self.entries.push(Entry {
            path: path.clone(),
            stamp,
            module,
            generation: 1,
            error: None,
        });
        Ok(ShaderId(self.entries.len() as u32 - 1))
    }

    fn entry(&self, id: ShaderId) -> Result<&Entry, GpuError> {
        self.entries
            .get(id.0 as usize)
            .ok_or_else(|| GpuError::UnknownShader(format!("{id:?}")))
    }

    /// The module in service (the last good version).
    pub fn module(&self, id: ShaderId) -> Result<&wgpu::ShaderModule, GpuError> {
        Ok(&self.entry(id)?.module)
    }

    /// Bumped on every successful reload (1 at load). Rebuild pipelines when it moves.
    #[must_use]
    pub fn generation(&self, id: ShaderId) -> u64 {
        self.entry(id).map_or(0, |e| e.generation)
    }

    /// The error from the most recent failed reload, cleared by the next good one.
    #[must_use]
    pub fn last_error(&self, id: ShaderId) -> Option<&GpuError> {
        self.entry(id).ok().and_then(|e| e.error.as_ref())
    }

    /// Number of shaders held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the library holds no shader.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Check every shader file for change and reload the changed ones. A quiet poll is one
    /// `stamps_of` call and nothing else.
    pub fn poll(
        &mut self,
        store: &dyn ProjectStore,
        device: &wgpu::Device,
    ) -> Result<Vec<ShaderEvent>, GpuError> {
        if self.entries.is_empty() {
            return Ok(Vec::new());
        }
        let paths: Vec<StorePath> = self.entries.iter().map(|e| e.path.clone()).collect();
        let stamps = store.stamps_of(&paths).map_err(|e| GpuError::ShaderIo {
            path: "(shader library)".into(),
            why: e.to_string(),
        })?;
        let mut events = Vec::new();
        for (i, e) in self.entries.iter_mut().enumerate() {
            let now = stamps.iter().find(|(p, _)| *p == e.path).map(|(_, s)| *s);
            if now == e.stamp {
                continue;
            }
            e.stamp = now;
            let id = ShaderId(i as u32);
            let path = e.path.as_str().to_string();
            let result = if now.is_none() {
                Err(GpuError::ShaderIo {
                    path: path.clone(),
                    why: "the file was deleted; the last good version stays in service".into(),
                })
            } else {
                read_source(store, &e.path).and_then(|src| compile_wgsl(device, &path, &src))
            };
            match result {
                Ok(m) => {
                    e.module = m;
                    e.generation += 1;
                    e.error = None;
                    events.push(ShaderEvent::Reloaded {
                        id,
                        path,
                        generation: e.generation,
                    });
                }
                Err(error) => {
                    e.error = Some(error.clone());
                    events.push(ShaderEvent::Failed { id, path, error });
                }
            }
        }
        Ok(events)
    }
}
