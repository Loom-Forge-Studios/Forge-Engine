//! `forge-store-backends` — the first-party store backends as a plugin (Ch.33.1, M2-14,
//! M2-15).
//!
//! `forge.store` provides, on `forge-store`'s public `StoreBackend` point:
//!
//! | Scheme | Store |
//! |---|---|
//! | `file` | [`LocalFs`]: a project is an ordinary folder (the default) |
//! | `memory` | [`MemoryStore`], labelled as not saved (D-4) |
//! | `git` | [`git::GitStore`] on a project folder: its history is a Git repository (`<folder>/.forge/git`) |
//! | `https`, `http` | a view of a Git remote over the smart HTTP protocol: GitHub, GitLab, Gitea, Forgejo |
//! | `git-file` | a view of a Git repository on a path: a bare repository on a share |
//!
//! `forge_store::open_store` sees only the registry, so a third-party plugin can add `s3`,
//! replace `file`, or chain it (a logging or caching store) on equal terms (I16, I17).
//!
//! ```
//! use forge_plugin::{Extensions, Grants, loader};
//! use forge_store::{StoreBackend, open_store};
//!
//! let mut x = Extensions::new();
//! x.define::<StoreBackend>()?;
//! loader::load(&mut x, &[&forge_store_backends::StoreBackends::new()?], &[], &Grants::new())?;
//! let store = open_store(&x, "memory:scratch", "ada").map_err(|e| e.to_string());
//! assert!(store.is_ok());
//! # Ok::<(), forge_plugin::PluginError>(())
//! ```

#![forbid(unsafe_code)]

pub mod git;

use std::path::PathBuf;
use std::sync::Arc;

use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};
use forge_store::{BackendDescriptor, LocalFs, MemoryStore, ProjectStore, StoreBackend};

use git::{GitOptions, GitStore};

const MANIFEST: &str = r#"Plugin(
    id: "forge.store",
    version: "0.1.0",
    engine: "^0.1",
    kind: Source,
    provides: [
        StoreBackend("file"), StoreBackend("memory"), StoreBackend("git"),
        StoreBackend("https"), StoreBackend("http"), StoreBackend("git-file"),
    ],
    capabilities: [ Fs(ProjectRead), Fs(ProjectWrite), Net(Outbound) ],
)"#;

/// The first-party store backends.
pub struct StoreBackends {
    manifest: Manifest,
    git: GitOptions,
}

impl StoreBackends {
    /// The plugin, with Git remotes mirrored under the per-user cache directory.
    pub fn new() -> Result<Self, PluginError> {
        Self::with_git(GitOptions::new())
    }

    /// The plugin with explicit Git options (the cache directory, the credentials the
    /// editor's sign-in fills, the HTTP client).
    pub fn with_git(git: GitOptions) -> Result<Self, PluginError> {
        Ok(Self {
            manifest: Manifest::parse(MANIFEST)?,
            git,
        })
    }

    /// The Git options every Git store this plugin opens uses.
    #[must_use]
    pub fn git_options(&self) -> &GitOptions {
        &self.git
    }
}

impl SourcePlugin for StoreBackends {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.add::<StoreBackend>(
            "file",
            BackendDescriptor {
                label: "Local folder".into(),
                open: Arc::new(|loc: &str, who: &str| {
                    LocalFs::open(PathBuf::from(loc), who)
                        .map(|s| Box::new(s) as Box<dyn ProjectStore>)
                }),
            },
            Order::First,
        )?;
        cx.add::<StoreBackend>(
            "memory",
            BackendDescriptor {
                label: "In memory (not saved; D-4)".into(),
                open: Arc::new(|_: &str, who: &str| {
                    Ok(Box::new(MemoryStore::new(who)) as Box<dyn ProjectStore>)
                }),
            },
            Order::Last,
        )?;
        cx.add::<StoreBackend>(
            "git",
            BackendDescriptor {
                label: "Local folder with Git history".into(),
                open: Arc::new(|loc: &str, who: &str| {
                    GitStore::open_folder(PathBuf::from(loc), who)
                        .map(|s| Box::new(s) as Box<dyn ProjectStore>)
                }),
            },
            Order::Last,
        )?;
        for (scheme, label) in [
            ("https", "Git remote (GitHub, GitLab, Gitea, Forgejo)"),
            (
                "http",
                "Git remote over plain HTTP (a server on your network)",
            ),
            (
                "git-file",
                "Git repository on a path (a bare repository on a share)",
            ),
        ] {
            let opts = self.git.clone();
            cx.add::<StoreBackend>(
                scheme,
                BackendDescriptor {
                    label: label.into(),
                    open: Arc::new(move |loc: &str, who: &str| {
                        GitStore::open_remote(&format!("{scheme}:{loc}"), who, &opts)
                            .map(|s| Box::new(s) as Box<dyn ProjectStore>)
                    }),
                },
                Order::Last,
            )?;
        }
        Ok(())
    }
}
