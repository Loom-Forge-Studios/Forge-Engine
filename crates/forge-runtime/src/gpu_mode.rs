//! The GPU mode an exported game starts in (WP-39, ADR 0054).
//!
//! The project's Graphics setting (`graphics.gpu_mode`) is written by the packager into the
//! `forge.json` beside the executable as `"gpu_mode": "single" | "multi"`. At startup the
//! game reads it (`--gpu-mode single|multi` on the command line overrides it) and builds its
//! one adapter pool in that mode; the pool is built once, so a change applies at the next
//! start. Anything missing or unreadable is **Single**, the default: a game never opens a
//! second GPU it was not told to.

use forge_ui::render_wgpu::GpuMode;

/// The file the packager writes beside the executable.
pub const FORGE_JSON: &str = "forge.json";

/// The mode `forge.json` asks for (Single when the file does not say, or is not JSON).
#[must_use]
pub fn from_forge_json(text: &str) -> GpuMode {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|j| j.get("gpu_mode")?.as_str().and_then(GpuMode::from_id))
        .unwrap_or_default()
}

/// The mode this run starts in: `--gpu-mode <id>` in `args`, else the `forge.json` in
/// `exe_dir`, else Single.
#[must_use]
pub fn startup(args: &[String], exe_dir: Option<&std::path::Path>) -> GpuMode {
    if let Some(m) = args
        .iter()
        .position(|a| a == "--gpu-mode")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| GpuMode::from_id(v))
    {
        return m;
    }
    exe_dir
        .and_then(|d| std::fs::read_to_string(d.join(FORGE_JSON)).ok())
        .map_or(GpuMode::Single, |t| from_forge_json(&t))
}

/// The directory of the running executable (where an exported game's `forge.json` is).
#[must_use]
pub fn exe_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_json_opts_in_and_everything_else_is_single() {
        assert_eq!(from_forge_json(r#"{"gpu_mode": "multi"}"#), GpuMode::Multi);
        assert_eq!(
            from_forge_json(r#"{"gpu_mode": "single"}"#),
            GpuMode::Single
        );
        assert_eq!(from_forge_json(r#"{"product": "x"}"#), GpuMode::Single);
        assert_eq!(from_forge_json(r#"{"gpu_mode": "both"}"#), GpuMode::Single);
        assert_eq!(from_forge_json("not json"), GpuMode::Single);
    }

    #[test]
    fn the_command_line_overrides_forge_json_beside_the_executable() {
        let dir = std::env::temp_dir().join(format!("forge-runtime-gpu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(startup(&[], Some(&dir)), GpuMode::Single, "no forge.json");
        std::fs::write(dir.join(FORGE_JSON), r#"{"gpu_mode": "multi"}"#)
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(startup(&[], Some(&dir)), GpuMode::Multi);
        let args = vec!["--gpu-mode".to_string(), "single".to_string()];
        assert_eq!(startup(&args, Some(&dir)), GpuMode::Single);
        assert_eq!(startup(&[], None), GpuMode::Single);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
