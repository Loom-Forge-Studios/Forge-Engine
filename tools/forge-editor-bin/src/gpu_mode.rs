//! The GPU mode the editor starts with (WP-39, ADR 0054): the user's Graphics preference,
//! read from their editor settings **before** the adapter pool is built — the pool is built
//! once, at startup, so a change in the settings window applies at the next start.

use forge_editor::settings::{EditorSettings, GpuModeSetting};
use forge_gpu::{GpuMode, PoolOptions};

/// The engine's mode for the preference.
#[must_use]
pub fn gpu_mode(setting: GpuModeSetting) -> GpuMode {
    match setting {
        GpuModeSetting::Single => GpuMode::Single,
        GpuModeSetting::Multi => GpuMode::Multi,
    }
}

/// The editor settings the pool is built from: the user's file, or the defaults when there
/// is no user config or the file is unreadable (the shell reports a bad file when it loads
/// it again; the GPU mode then falls back to Single, the default).
#[must_use]
pub fn startup_settings(config_dir: Option<&std::path::Path>) -> EditorSettings {
    config_dir
        .and_then(|d| EditorSettings::load(d).ok())
        .unwrap_or_default()
}

/// The options of the editor's one adapter pool (UI, viewport renderer, Tier-0 compute).
#[must_use]
pub fn pool_options(settings: &EditorSettings) -> PoolOptions {
    PoolOptions {
        mode: gpu_mode(settings.gpu_mode),
        ..PoolOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preference_reaches_the_pool_options() {
        assert_eq!(
            pool_options(&EditorSettings::default()).mode,
            GpuMode::Single
        );
        assert_eq!(startup_settings(None).gpu_mode, GpuModeSetting::Single);
        let dir =
            std::env::temp_dir().join(format!("forge-editor-gpu-mode-{}", std::process::id()));
        let s = EditorSettings {
            gpu_mode: GpuModeSetting::Multi,
            ..EditorSettings::default()
        };
        s.save(&dir).unwrap_or_else(|e| panic!("{e}"));
        let loaded = startup_settings(Some(&dir));
        assert_eq!(pool_options(&loaded).mode, GpuMode::Multi);
        // An unreadable file starts in Single, the default.
        std::fs::write(
            dir.join(forge_editor::settings::EDITOR_SETTINGS_FILE),
            "(((",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            pool_options(&startup_settings(Some(&dir))).mode,
            GpuMode::Single
        );
        let _ = std::fs::remove_dir_all(&dir);
        for s in [GpuModeSetting::Single, GpuModeSetting::Multi] {
            assert_eq!(gpu_mode(s).id(), s.id(), "one id for both layers");
        }
    }
}
