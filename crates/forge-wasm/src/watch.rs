//! Hot reload: poll plugin directories and swap changed code in place.
//!
//! A poll costs one `metadata` call per watched file and reads nothing while nothing has
//! changed (no thread, no wakeup: the editor calls [`PluginWatcher::poll`] from the loop it
//! already runs, so an idle editor stays idle, D-5). When a file's size or modification time
//! changes it is read once; identical content (a touch, a rewrite of the same bytes) is not
//! a reload.
//!
//! * **Code changed** (`plugin.wasm` / `plugin.wat`): compiled, checked against the
//!   unchanged manifest, instantiated, and swapped under every installed item
//!   ([`ReloadEvent::Reloaded`]). Items keep their registry places, owners and chains.
//! * **The new code fails** to compile, link or instantiate: the old code keeps serving
//!   ([`ReloadEvent::Failed`]) — a half-saved file never takes a plugin down.
//! * **Manifest changed**: declarations are what the loader checked for conflicts, so they
//!   never change underneath it: [`ReloadEvent::NeedsLoad`] asks the host to reload the plugin
//!   set through the loader (`WASM-0011`).
//! * **The host does not vouch for the new code** ([`PluginWatcher::poll_vetted`], WP-36):
//!   the bytes read are shown to the host's vet *before* they are compiled — the exact bytes
//!   that would run, under the manifest in effect — and a refusal keeps the old code serving
//!   ([`ReloadEvent::Held`]) without taking the new stamps, so the change is read and vetted
//!   again once the host allows it. There is no second read between the vet and the swap.

use std::path::{Path, PathBuf};

use forge_plugin::PluginId;

use crate::files::{PluginFiles, Stamp, hash, stamp};
use crate::host::{code_file, read_file};
use crate::{WasmError, WasmPlugin};

/// What a poll found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReloadEvent {
    /// New code is live.
    Reloaded {
        /// The plugin.
        plugin: PluginId,
        /// Its new generation.
        generation: u64,
    },
    /// The new code was refused; the old code is still live.
    Failed {
        /// The plugin.
        plugin: PluginId,
        /// Why.
        error: WasmError,
    },
    /// The manifest changed: reload the plugin set through the loader.
    NeedsLoad {
        /// The plugin.
        plugin: PluginId,
        /// `WASM-0011`.
        error: WasmError,
    },
    /// The host's vet refused the new code (WP-36): the old code is still live, and the
    /// change waits, unstamped, until the host allows it.
    Held {
        /// The plugin.
        plugin: PluginId,
        /// Its directory.
        dir: PathBuf,
    },
}

struct Watched {
    dir: PathBuf,
    plugin: WasmPlugin,
    /// The manifest in effect, as bytes (what a vet of new code is shown with it).
    manifest: Vec<u8>,
    manifest_stamp: Option<Stamp>,
    manifest_hash: u64,
    code_path: Option<PathBuf>,
    code_stamp: Option<Stamp>,
    code_hash: u64,
    /// Set after a `NeedsLoad`: reported once, then quiet until the host reloads.
    stale: bool,
}

forge_trace::control_switches! {
    /// W2 positive-control switches for the watcher's guards. Never set outside them.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct WatchFaults {
        /// The read-once guard's control (WP-36): after the vet allows a change, the code file
        /// is read **again** and that second read is compiled, as a watcher that re-reads
        /// would — a write between the vet and the compile then runs bytes nobody vetted.
        pub reread_after_vet: bool,
    }
}

/// Watches plugin directories for changes. See the module docs.
#[derive(Default)]
pub struct PluginWatcher {
    watched: Vec<Watched>,
    reads: usize,
    #[doc(hidden)]
    pub faults: WatchFaults,
}

impl std::fmt::Debug for PluginWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginWatcher")
            .field(
                "dirs",
                &self.watched.iter().map(|w| &w.dir).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl PluginWatcher {
    /// Watch nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Watch `dir`, which `plugin` was loaded from. Its baseline is the files the plugin was
    /// compiled from when it was loaded from files ([`crate::WasmHost::load_files`], WP-36:
    /// nothing is read again, so a write after the load is a change the next poll sees);
    /// otherwise the directory's current files. Watching a directory again replaces the old
    /// entry (after the host reloaded it).
    pub fn watch(&mut self, dir: &Path, plugin: &WasmPlugin) -> Result<(), WasmError> {
        let b = match plugin.baseline() {
            Some(b) => b.clone(),
            None => PluginFiles::read(dir)?.baseline(),
        };
        let w = Watched {
            dir: dir.to_path_buf(),
            plugin: plugin.clone(),
            manifest_hash: hash(&b.manifest),
            manifest: b.manifest,
            manifest_stamp: b.manifest_stamp,
            code_path: b.code_path,
            code_stamp: b.code_stamp,
            code_hash: b.code_hash,
            stale: false,
        };
        self.watched.retain(|x| x.dir != w.dir);
        self.watched.push(w);
        Ok(())
    }

    /// Stop watching `dir`.
    pub fn unwatch(&mut self, dir: &Path) {
        self.watched.retain(|x| x.dir != dir);
    }

    /// How many files the last poll read (0 when nothing changed).
    #[must_use]
    pub fn last_poll_reads(&self) -> usize {
        self.reads
    }

    /// Check every watched directory once. Cheap when nothing changed.
    pub fn poll(&mut self) -> Vec<ReloadEvent> {
        self.poll_vetted(|_| true, |_, _| true)
    }

    /// [`PluginWatcher::poll`], skipping the directories `may` refuses: their changes wait,
    /// unread (their stamps are not taken), so a later poll that allows them reloads them as
    /// it would have. The editor holds a project plugin's reload this way while the person has
    /// not trusted its changed files (WP-35).
    pub fn poll_where(&mut self, may: impl Fn(&Path) -> bool) -> Vec<ReloadEvent> {
        self.poll_vetted(may, |_, _| true)
    }

    /// [`PluginWatcher::poll_where`], showing each changed code file to `vet` before it is
    /// compiled (WP-36): `vet(dir, files)` sees the manifest in effect and the new code bytes
    /// — exactly what would run — and a refusal keeps the old code ([`ReloadEvent::Held`])
    /// and leaves the change unstamped for a later poll.
    pub fn poll_vetted(
        &mut self,
        may: impl Fn(&Path) -> bool,
        mut vet: impl FnMut(&Path, &PluginFiles) -> bool,
    ) -> Vec<ReloadEvent> {
        self.reads = 0;
        let reread = self.faults.reread_after_vet();
        let mut out = Vec::new();
        for w in &mut self.watched {
            if w.stale || !may(&w.dir) {
                continue;
            }
            let id = w.plugin.id().clone();
            // Manifest first: a changed declaration wins over a code change.
            let manifest_path = w.dir.join("plugin.ron");
            let ms = stamp(&manifest_path);
            if ms != w.manifest_stamp {
                w.manifest_stamp = ms;
                self.reads += 1;
                if let Ok(bytes) = read_file(&manifest_path) {
                    let h = hash(&bytes);
                    if h != w.manifest_hash {
                        w.manifest_hash = h;
                        w.stale = true;
                        out.push(ReloadEvent::NeedsLoad {
                            plugin: id.clone(),
                            error: WasmError::ManifestChanged {
                                plugin: id.to_string(),
                            },
                        });
                        continue;
                    }
                }
            }
            let code_path = code_file(&w.dir);
            let cs = code_path.as_deref().and_then(stamp);
            if code_path == w.code_path && cs == w.code_stamp {
                continue;
            }
            let Some(path) = code_path else {
                w.code_path = None;
                w.code_stamp = None;
                out.push(ReloadEvent::Failed {
                    plugin: id,
                    error: WasmError::Io {
                        path: w.dir.display().to_string(),
                        why: "plugin.wasm / plugin.wat is gone; the loaded code keeps running"
                            .into(),
                    },
                });
                continue;
            };
            self.reads += 1;
            let bytes = match read_file(&path) {
                Ok(b) => b,
                Err(error) => {
                    w.code_path = Some(path);
                    w.code_stamp = cs;
                    out.push(ReloadEvent::Failed { plugin: id, error });
                    continue;
                }
            };
            let h = hash(&bytes);
            if h == w.code_hash {
                // Same bytes: a touch, not a change.
                w.code_path = Some(path);
                w.code_stamp = cs;
                continue;
            }
            let files = PluginFiles::from_parts(w.manifest.clone(), Some((path.clone(), bytes)));
            if !vet(&w.dir, &files) {
                // Not vouched for: the old code keeps serving and the stamps stay as they
                // were, so the change is read (and vetted) again when the host allows it.
                out.push(ReloadEvent::Held {
                    plugin: id,
                    dir: w.dir.clone(),
                });
                continue;
            }
            // The vetted bytes are the ones compiled: nothing is read between the vet and
            // the swap (the control reads again, as a re-reading watcher would).
            let again = if reread {
                read_file(&path).unwrap_or_default()
            } else {
                Vec::new()
            };
            w.code_path = Some(path);
            w.code_stamp = cs;
            let code = if reread {
                again.as_slice()
            } else {
                files.code().map(|(_, b)| b).unwrap_or_default()
            };
            match w.plugin.reload(code) {
                Ok(generation) => {
                    w.code_hash = h;
                    out.push(ReloadEvent::Reloaded {
                        plugin: id,
                        generation,
                    });
                }
                Err(error) => out.push(ReloadEvent::Failed { plugin: id, error }),
            }
        }
        out
    }
}
