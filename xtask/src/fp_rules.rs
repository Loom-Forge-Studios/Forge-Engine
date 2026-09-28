//! `cargo xtask fp-rules` — Ch.3.3 build-level floating-point rules, asserted.
//!
//! * No fast-math equivalents.
//! * No `-C target-feature` (it can change FP lowering, e.g. `+fma` contraction).
//! * No `-C target-cpu=...` in any artefact-producing configuration — `native` changes SIMD
//!   lowering and therefore reduction order; any named CPU does the same thing on purpose.
//! * No `-C llvm-args` at all: every LLVM FP knob (`-fp-contract=fast`,
//!   `-enable-unsafe-fp-math`, `-enable-no-nans-fp-math`, ...) arrives that way, and nothing
//!   in this engine needs any other LLVM flag.
//!
//! Checked sources: `.cargo/config.toml` (which must pin `build.rustflags`), every
//! `rustflags`-shaped key in the workspace `Cargo.toml`, and the environment variables cargo
//! reads flags from, so a CI runner or a developer shell cannot slip them in either.

use std::path::Path;

use crate::util::{Failure, XResult, read};

pub const CONFIG_FILE: &str = ".cargo/config.toml";
pub const MANIFEST: &str = "Cargo.toml";

/// Check one list of rustc flags. Returns the violations, each naming `origin`.
pub fn check_flags(flags: &[String], origin: &str) -> Vec<String> {
    // Normalise `-C x` (two entries) into `-Cx`, and `--codegen x` likewise.
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        let f = flags[i].trim();
        if (f == "-C" || f == "--codegen") && i + 1 < flags.len() {
            tokens.push(format!("-C{}", flags[i + 1].trim()));
            i += 2;
            continue;
        }
        if let Some(rest) = f.strip_prefix("--codegen=") {
            tokens.push(format!("-C{rest}"));
        } else {
            tokens.push(f.to_string());
        }
        i += 1;
    }
    let mut errs = Vec::new();
    for t in &tokens {
        let lower = t.to_ascii_lowercase();
        let why = if lower.starts_with("-ctarget-cpu") {
            Some("target-cpu changes SIMD lowering and therefore reduction order")
        } else if lower.starts_with("-ctarget-feature") {
            Some("target-feature can change FP lowering (e.g. +fma contraction)")
        } else if lower.starts_with("-cllvm-args") {
            Some("llvm-args is how fast-math and FP-contraction knobs arrive")
        } else if [
            "fast-math",
            "unsafe-fp-math",
            "fp-contract",
            "no-nans",
            "no-infs",
            "no-signed-zeros",
            "unsafe-math",
        ]
        .iter()
        .any(|p| lower.contains(p))
        {
            Some("fast-math equivalents are forbidden")
        } else {
            None
        };
        if let Some(why) = why {
            errs.push(format!(
                "{origin}: forbidden rustc flag `{t}` — {why} (Ch.3.3)"
            ));
        }
    }
    errs
}

/// Walk a TOML document and check every value under a key named `rustflags` / `RUSTFLAGS`.
fn walk(v: &toml::Value, path: &str, origin: &str, errs: &mut Vec<String>) {
    if let toml::Value::Table(t) = v {
        for (k, child) in t {
            let p = if path.is_empty() {
                k.clone()
            } else {
                format!("{path}.{k}")
            };
            let key = k.to_ascii_lowercase();
            if key == "rustflags" || key.ends_with("_rustflags") {
                let flags = flag_list(child);
                errs.extend(check_flags(&flags, &format!("{origin} [{p}]")));
            } else {
                walk(child, &p, origin, errs);
            }
        }
    }
}

fn flag_list(v: &toml::Value) -> Vec<String> {
    match v {
        toml::Value::String(s) => s.split_whitespace().map(str::to_string).collect(),
        toml::Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        // `[env] RUSTFLAGS = { value = "..." }`
        toml::Value::Table(t) => t.get("value").map(flag_list).unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn check_config(src: &str, origin: &str, require_pin: bool) -> XResult<()> {
    let doc: toml::Value =
        toml::from_str(src).map_err(|e| Failure::one(format!("{origin}: parse error: {e}")))?;
    let mut errs = Vec::new();
    if require_pin {
        let pinned = doc.get("build").and_then(|b| b.get("rustflags")).is_some();
        if !pinned {
            errs.push(format!(
                "{origin}: `build.rustflags` must be pinned explicitly (Ch.3.3: pinned, not relied on)"
            ));
        }
    }
    walk(&doc, "", origin, &mut errs);
    Failure::many(errs)
}

/// Flags from the environment variables cargo honours.
pub fn check_env(vars: &[(String, String)]) -> XResult<()> {
    let mut errs = Vec::new();
    for (k, v) in vars {
        let flags: Vec<String> = if k == "CARGO_ENCODED_RUSTFLAGS" {
            v.split('\u{1f}')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        } else if k == "RUSTFLAGS"
            || k == "CARGO_BUILD_RUSTFLAGS"
            || (k.starts_with("CARGO_TARGET_") && k.ends_with("_RUSTFLAGS"))
        {
            v.split_whitespace().map(str::to_string).collect()
        } else {
            continue;
        };
        errs.extend(check_flags(&flags, &format!("environment variable {k}")));
    }
    Failure::many(errs)
}

pub fn run(root: &Path) -> XResult<String> {
    let mut errs = Vec::new();
    for (file, pin) in [(CONFIG_FILE, true), (MANIFEST, false)] {
        match read(&root.join(file)).and_then(|s| check_config(&s, file, pin)) {
            Ok(()) => {}
            Err(e) => errs.extend(e.messages),
        }
    }
    let vars: Vec<(String, String)> = std::env::vars().collect();
    if let Err(e) = check_env(&vars) {
        errs.extend(e.messages);
    }
    Failure::many(errs)?;
    Ok(
        "fp-rules: no target-cpu, target-feature, llvm-args or fast-math flags anywhere (Ch.3.3)\n"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::workspace_root;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn committed_config_passes() {
        run(&workspace_root()).expect("committed config must satisfy Ch.3.3");
    }

    #[test]
    fn harmless_flags_pass() {
        assert!(check_flags(&s(&["-Cdebuginfo=1", "-Dwarnings"]), "t").is_empty());
    }

    // ---- positive controls ----

    #[test]
    fn positive_control_target_cpu_native_fails_in_every_spelling() {
        assert!(!check_flags(&s(&["-Ctarget-cpu=native"]), "t").is_empty());
        assert!(!check_flags(&s(&["-C", "target-cpu=native"]), "t").is_empty());
        assert!(!check_flags(&s(&["--codegen=target-cpu=native"]), "t").is_empty());
        let cfg = "[build]\nrustflags = []\n[target.x86_64-pc-windows-msvc]\nrustflags = [\"-C\", \"target-cpu=native\"]\n";
        let err = check_config(cfg, "cfg", true).unwrap_err();
        assert!(err.to_string().contains("target-cpu=native"), "{err}");
    }

    #[test]
    fn positive_control_fast_math_and_fma_fail() {
        assert!(!check_flags(&s(&["-Cllvm-args=-fp-contract=fast"]), "t").is_empty());
        assert!(!check_flags(&s(&["-Ctarget-feature=+fma"]), "t").is_empty());
        assert!(!check_flags(&s(&["-Zunsafe-math-opts"]), "t").is_empty());
        let cfg = "[build]\nrustflags = \"-C llvm-args=-enable-unsafe-fp-math\"\n";
        assert!(check_config(cfg, "cfg", true).is_err());
    }

    #[test]
    fn positive_control_unpinned_config_fails() {
        let err = check_config("[alias]\nx = \"run\"\n", "cfg", true).unwrap_err();
        assert!(err.to_string().contains("must be pinned"), "{err}");
    }

    #[test]
    fn positive_control_env_flags_fail() {
        let vars = vec![
            ("RUSTFLAGS".to_string(), "-C target-cpu=native".to_string()),
            (
                "CARGO_ENCODED_RUSTFLAGS".to_string(),
                "-C\u{1f}target-feature=+avx2".to_string(),
            ),
            ("PATH".to_string(), "-Ctarget-cpu=native".to_string()),
        ];
        let err = check_env(&vars).unwrap_err();
        assert_eq!(err.messages.len(), 2, "{err}");
    }

    #[test]
    fn positive_control_env_table_in_config_fails() {
        let cfg = "[build]\nrustflags = []\n[env]\nRUSTFLAGS = { value = \"-Ctarget-cpu=native\", force = true }\n";
        assert!(check_config(cfg, "cfg", true).is_err());
    }
}
