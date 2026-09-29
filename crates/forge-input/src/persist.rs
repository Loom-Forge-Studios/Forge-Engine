//! Rebinding persistence (Ch.28 §28.13): a player's binding overrides saved per profile and
//! loaded at the next start. The file holds **only what the player changed** (per action
//! path and slot, the binding text or `None` for unbound), so a game update that changes a
//! default the player never touched reaches them, and an override of an action the update
//! removed is kept (not applied) in case the action comes back.
//!
//! The file store writes atomically (a temporary file renamed over the old one), so a crash
//! mid-save never leaves a half-written file.

use std::path::{Path, PathBuf};

use crate::control::Binding;
use crate::error::InputError;
use crate::map::OverridesFile;
use crate::runtime::{BindingOverrides, InputRuntime, PlayerId};

/// The current file version.
pub const OVERRIDES_VERSION: u32 = 1;

impl BindingOverrides {
    pub fn to_file(&self) -> OverridesFile {
        OverridesFile {
            version: OVERRIDES_VERSION,
            bindings: self
                .map
                .iter()
                .filter(|(_, s)| !s.is_empty())
                .map(|(a, s)| {
                    (
                        a.clone(),
                        s.iter()
                            .map(|(n, b)| (*n, b.as_ref().map(ToString::to_string)))
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    /// From a file's form; bindings that do not parse are skipped and named.
    pub fn from_file(f: &OverridesFile) -> (BindingOverrides, Vec<String>) {
        let mut o = BindingOverrides::default();
        let mut problems = Vec::new();
        for (a, slots) in &f.bindings {
            for (n, b) in slots {
                match b {
                    None => o.set(a, *n, None),
                    Some(t) => match Binding::parse(t) {
                        Ok(b) => o.set(a, *n, Some(b)),
                        Err(e) => problems.push(format!("{a} slot {n}: {e}")),
                    },
                }
            }
        }
        (o, problems)
    }

    pub fn to_ron(&self) -> Result<String, InputError> {
        ron::ser::to_string_pretty(&self.to_file(), ron::ser::PrettyConfig::default())
            .map_err(|e| InputError::BadOverrides(e.to_string()))
    }

    pub fn from_ron(text: &str) -> Result<(BindingOverrides, Vec<String>), InputError> {
        let f: OverridesFile =
            ron::from_str(text).map_err(|e| InputError::BadOverrides(e.to_string()))?;
        if f.version > OVERRIDES_VERSION {
            return Err(InputError::BadOverrides(format!(
                "version {} is newer than this build reads ({OVERRIDES_VERSION})",
                f.version
            )));
        }
        Ok(BindingOverrides::from_file(&f))
    }
}

/// Where overrides are kept.
pub trait OverrideStore {
    fn load(&self, profile: &str) -> Result<Option<BindingOverrides>, InputError>;
    fn save(&self, profile: &str, o: &BindingOverrides) -> Result<(), InputError>;
}

fn check_profile(profile: &str) -> Result<(), InputError> {
    if profile.is_empty()
        || profile.len() > 64
        || !profile
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(InputError::BadProfile(profile.to_string()));
    }
    Ok(())
}

/// One RON file per profile in a directory (`<dir>/<profile>.input.ron`), written
/// atomically.
#[derive(Clone, Debug)]
pub struct FileOverrideStore {
    dir: PathBuf,
}

impl FileOverrideStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn path(&self, profile: &str) -> PathBuf {
        self.dir.join(format!("{profile}.input.ron"))
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl OverrideStore for FileOverrideStore {
    fn load(&self, profile: &str) -> Result<Option<BindingOverrides>, InputError> {
        check_profile(profile)?;
        let p = self.path(profile);
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(InputError::Io {
                    path: p.display().to_string(),
                    detail: e.to_string(),
                });
            }
        };
        let (o, _problems) = BindingOverrides::from_ron(&text)?;
        Ok(Some(o))
    }

    fn save(&self, profile: &str, o: &BindingOverrides) -> Result<(), InputError> {
        check_profile(profile)?;
        let io = |p: &Path, e: std::io::Error| InputError::Io {
            path: p.display().to_string(),
            detail: e.to_string(),
        };
        std::fs::create_dir_all(&self.dir).map_err(|e| io(&self.dir, e))?;
        let p = self.path(profile);
        let tmp = self.dir.join(format!("{profile}.input.ron.tmp"));
        std::fs::write(&tmp, o.to_ron()?).map_err(|e| io(&tmp, e))?;
        std::fs::rename(&tmp, &p).map_err(|e| io(&p, e))
    }
}

/// An in-memory store (tests, a console's save-data layer before it is wired).
#[derive(Debug, Default)]
pub struct MemoryOverrideStore {
    files: std::cell::RefCell<std::collections::BTreeMap<String, String>>,
}

impl MemoryOverrideStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl OverrideStore for MemoryOverrideStore {
    fn load(&self, profile: &str) -> Result<Option<BindingOverrides>, InputError> {
        check_profile(profile)?;
        match self.files.borrow().get(profile) {
            None => Ok(None),
            Some(t) => Ok(Some(BindingOverrides::from_ron(t)?.0)),
        }
    }
    fn save(&self, profile: &str, o: &BindingOverrides) -> Result<(), InputError> {
        check_profile(profile)?;
        self.files
            .borrow_mut()
            .insert(profile.to_string(), o.to_ron()?);
        Ok(())
    }
}

impl InputRuntime {
    /// Save a player's overrides to `profile`.
    pub fn save_bindings(
        &self,
        p: PlayerId,
        store: &dyn OverrideStore,
        profile: &str,
    ) -> Result<(), InputError> {
        let o = self.overrides(p).ok_or(InputError::NoPlayer(p.0))?;
        store.save(profile, o)
    }

    /// Load `profile`'s overrides into a player (none saved: the defaults).
    pub fn load_bindings(
        &mut self,
        p: PlayerId,
        store: &dyn OverrideStore,
        profile: &str,
    ) -> Result<bool, InputError> {
        if self.overrides(p).is_none() {
            return Err(InputError::NoPlayer(p.0));
        }
        match store.load(profile)? {
            Some(o) => {
                self.set_overrides(p, o);
                Ok(true)
            }
            None => Ok(false),
        }
    }
}
