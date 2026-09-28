//! The launcher's **recent projects** and default projects folder — user config (Ch.21
//! §21.18): per user, never in a project, written through the crash-safe user-config writer.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::EditorError;
use crate::user_config::{read_optional, write_atomic};

/// The file, under the user config directory.
pub const RECENT_FILE: &str = "recent_projects.ron";
/// Entries kept.
pub const RECENT_MAX: usize = 12;

/// One recent project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEntry {
    /// `file:<folder>` or `memory:<name>`.
    pub location: String,
    pub name: String,
    /// The template it was made from, if known.
    #[serde(default)]
    pub template: String,
}

/// The list, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentProjects {
    pub entries: Vec<RecentEntry>,
}

impl RecentProjects {
    /// Load from `dir` (missing file: empty; a bad file is an error the caller reports).
    pub fn load(dir: &Path) -> Result<Self, EditorError> {
        match read_optional(&dir.join(RECENT_FILE))? {
            Some(t) => {
                ron::from_str(&t).map_err(|e| EditorError::Io(format!("{RECENT_FILE}: {e}")))
            }
            None => Ok(Self::default()),
        }
    }

    /// Save to `dir`, crash-safely.
    pub fn save(&self, dir: &Path) -> Result<(), EditorError> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new())
            .map_err(|e| EditorError::Io(format!("{RECENT_FILE}: {e}")))?;
        write_atomic(&dir.join(RECENT_FILE), text.as_bytes())
    }

    /// Put `location` first (updating its name), keeping at most [`RECENT_MAX`]. Returns
    /// whether the list changed.
    pub fn touch(&mut self, location: &str, name: &str, template: &str) -> bool {
        let e = RecentEntry {
            location: location.to_string(),
            name: name.to_string(),
            template: template.to_string(),
        };
        if self.entries.first() == Some(&e) {
            return false;
        }
        self.entries.retain(|x| x.location != location);
        self.entries.insert(0, e);
        self.entries.truncate(RECENT_MAX);
        true
    }

    /// Forget a location (the launcher's "Remove from list").
    pub fn forget(&mut self, location: &str) -> bool {
        let n = self.entries.len();
        self.entries.retain(|x| x.location != location);
        self.entries.len() != n
    }
}

/// The launcher's user-side state: the recent list, where new projects go by default, and
/// where user config lives (`None`: nothing is saved — tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherState {
    pub recent: RecentProjects,
    pub projects_dir: PathBuf,
    pub config_dir: Option<PathBuf>,
    revision: u64,
}

impl Default for LauncherState {
    fn default() -> Self {
        Self {
            recent: RecentProjects::default(),
            projects_dir: default_projects_dir(),
            config_dir: None,
            revision: 0,
        }
    }
}

/// `<Documents>/Forge Projects` (`%USERPROFILE%` or `$HOME`), else the current directory.
pub fn default_projects_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Documents").join("Forge Projects")
}

impl LauncherState {
    /// With `config_dir`, the recent list loaded from it (a bad file starts empty; the
    /// error is returned for the caller to report).
    pub fn with_config(config_dir: Option<PathBuf>) -> (Self, Option<EditorError>) {
        let mut s = Self {
            config_dir,
            ..Self::default()
        };
        let mut err = None;
        if let Some(d) = &s.config_dir {
            match RecentProjects::load(d) {
                Ok(r) => s.recent = r,
                Err(e) => err = Some(e),
            }
        }
        (s, err)
    }

    /// Bumped whenever the list changes (the launcher's sync step compares it).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Record a project as just used, and save the list.
    pub fn touch(&mut self, location: &str, name: &str, template: &str) -> Result<(), EditorError> {
        if self.recent.touch(location, name, template) {
            self.revision += 1;
            if let Some(d) = &self.config_dir {
                self.recent.save(d)?;
            }
        }
        Ok(())
    }

    /// Forget a location, and save the list.
    pub fn forget(&mut self, location: &str) -> Result<(), EditorError> {
        if self.recent.forget(location) {
            self.revision += 1;
            if let Some(d) = &self.config_dir {
                self.recent.save(d)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_projects_are_newest_first_deduplicated_capped_and_saved() {
        let dir = std::env::temp_dir().join(format!("forge-recent-{}", std::process::id()));
        let (mut s, err) = LauncherState::with_config(Some(dir.clone()));
        assert!(err.is_none());
        for i in 0..20 {
            s.touch(&format!("file:p{i}"), &format!("P{i}"), "3d")
                .unwrap_or_else(|e| panic!("{e}"));
        }
        s.touch("file:p5", "P5 renamed", "3d")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(s.recent.entries.len(), RECENT_MAX);
        assert_eq!(s.recent.entries[0].name, "P5 renamed");
        assert_eq!(
            s.recent
                .entries
                .iter()
                .filter(|e| e.location == "file:p5")
                .count(),
            1
        );
        let (back, _) = LauncherState::with_config(Some(dir.clone()));
        assert_eq!(back.recent, s.recent);
        let rev = s.revision();
        s.forget("file:p5").unwrap_or_else(|e| panic!("{e}"));
        assert!(s.revision() > rev);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
