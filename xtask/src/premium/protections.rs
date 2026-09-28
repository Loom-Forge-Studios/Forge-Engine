//! `cargo xtask premium-protections` — the hardened-release guard (ADR 0063, WP-45): the
//! shipped premium binaries carry **no build-machine source paths** and are built with the
//! hardened profile (symbols stripped, no debug info, fat LTO, one codegen unit; paths remapped
//! out via `--remap-path-prefix`).
//!
//! Two layers, so the guard has a real positive control:
//!
//! 1. **The profile is hardened.** `release-premium` in the root `Cargo.toml` must set every
//!    hardening key ([`required_profile_keys`]). This is a fast, deterministic file check.
//! 2. **A built binary is clean, and the check can detect a dirty one.** The command builds a
//!    premium binary with `--profile release-premium` and checks it carries no build-machine
//!    paths ([`scan`]), no plain-text licensing strings ([`required_licence_needles`]), no debug
//!    symbols ([`symbol_findings`]), and a **non-dev** public key — not the forgeable committed
//!    dev key ([`dev_key_needle`]) but the operator's `FORGE_LICENCE_PUBKEY`, or the fixed check
//!    key ([`CHECK_PUBLIC_KEY_HEX`]) it injects so the check is self-contained (WP-45 problem 3).
//!    Its **positive control** is an ordinary (`dev`) build of the same binary, which does carry
//!    the paths, the plain strings and the dev key — proving each scan is not vacuous.
//!
//! **Build-machine paths** (WP-47, the WP-45 verifier's follow-up b): the workspace root, the home
//! directory **and the cargo target directory** are remapped ([`remap_prefixes`]: the target
//! directory and the build scripts' `OUT_DIR` parent too, deepest last, since rustc applies the
//! last matching remap), and the scan looks for each of them and for what a shallower remap
//! would leave of the deeper ones ([`path_needles`]: `~\ForgeTargets\<lane>` is what a home-only
//! remap left of cranelift's generated-code paths). `--drop-target-remap` is the break test: the
//! same run without the target-directory remaps must fail.
//!
//! The pure [`scan`], [`symbol_findings`] and [`parse_key_hex`] are unit-tested on synthetic
//! buffers (a buffer holding a path/symbol/key is caught, a clean one is not), so each control
//! "actually fails" without a multi-minute build.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util::{Failure, XResult, read};

/// The profile the shipped premium binaries build with.
pub const PROFILE: &str = "release-premium";

/// The `[profile.release-premium]` keys the hardened profile must set, each with the value the
/// guard requires (a substring of the line after `key =`).
#[must_use]
pub fn required_profile_keys() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("inherits", vec!["\"release\""]),
        ("strip", vec!["\"symbols\"", "true"]),
        ("debug", vec!["false", "0"]),
        ("lto", vec!["\"fat\"", "true"]),
        ("codegen-units", vec!["1"]),
    ]
}

/// The user's home directories (`USERPROFILE`, `HOME`), as the environment names them.
fn homes() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for var in ["USERPROFILE", "HOME"] {
        if let Ok(home) = std::env::var(var)
            && !home.is_empty()
            && !out.contains(&home)
        {
            out.push(home);
        }
    }
    out
}

/// The build-machine path needles a clean binary must not contain (see [`path_needles`]) for
/// this machine: the workspace root, the cargo target directory and the user's home.
#[must_use]
pub fn build_path_needles(root: &Path) -> Vec<String> {
    path_needles(root, &target_dir(root), &homes())
}

/// The build-machine path needles, pure (WP-45 follow-up b, WP-47): the absolute workspace
/// `root`, the absolute cargo `target` directory (where build scripts' `OUT_DIR` lives: code a
/// build script generates — cranelift's ISLE output — embeds its path in panic locations), and
/// each `home` directory. And the **remapped leftovers**: a path under another remapped prefix
/// comes out of `--remap-path-prefix` as that prefix's token plus the rest of the path — a
/// target directory under the home directory becomes `~\ForgeTargets\<lane>` when only the home
/// is remapped — which still names this machine's layout, so each such form is a needle too.
/// Matching is separator- and case-insensitive ([`scan_paths`]).
#[must_use]
pub fn path_needles(root: &Path, target: &Path, homes: &[String]) -> Vec<String> {
    let root_s = root.display().to_string();
    let mut needles = forms(&root_s, root, homes);
    needles.extend(homes.iter().cloned());
    needles.extend(target_needles(root, target, homes));
    let mut seen = std::collections::BTreeSet::new();
    needles.retain(|n| !n.is_empty() && seen.insert(normalise_path(n.as_bytes())));
    needles
}

/// The target directory's needles alone: its absolute path and its remapped leftovers (see
/// [`path_needles`]). The dev control build must contain one of them (its build scripts'
/// output paths), or the target-dir check would prove nothing.
#[must_use]
pub fn target_needles(root: &Path, target: &Path, homes: &[String]) -> Vec<String> {
    forms(&target.display().to_string(), root, homes)
}

/// `path` and the forms another remapped prefix leaves of it: the prefix's token plus the rest
/// (the tokens are [`remap_prefixes`]' for the home directory and the workspace root).
fn forms(path: &str, root: &Path, homes: &[String]) -> Vec<String> {
    let mut prefixes: Vec<(String, &str)> = homes.iter().map(|h| (h.clone(), "~")).collect();
    prefixes.push((root.display().to_string(), "/forge"));
    let mut out = vec![path.to_string()];
    for (prefix, token) in &prefixes {
        if let Some(rest) = strip_path_prefix(path, prefix)
            && !rest.is_empty()
        {
            out.push(format!("{token}{rest}"));
        }
    }
    out
}

/// `path` without `prefix` (component-wise: `C:\a` is not a prefix of `C:\ab`), separator- and
/// case-insensitive; the rest keeps its leading separator.
fn strip_path_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let (p, q) = (
        normalise_path(path.as_bytes()),
        normalise_path(prefix.as_bytes()),
    );
    let q = q.strip_suffix(b"/").unwrap_or(&q);
    if p.len() > q.len() && p.starts_with(q) && p[q.len()] == b'/' {
        path.get(q.len()..)
    } else {
        None
    }
}

/// A path's bytes with `\` as `/` and ASCII letters lower-cased (Windows paths are neither
/// separator- nor case-sensitive, and an embedded path may mix separators: `~\ForgeTargets/B`).
fn normalise_path(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|&b| {
            if b == b'\\' {
                b'/'
            } else {
                b.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Scan `bytes` for each path needle, separator- and case-insensitively; the needles found
/// (empty means clean). Pure.
#[must_use]
pub fn scan_paths(bytes: &[u8], needles: &[String]) -> Vec<String> {
    let hay = normalise_path(bytes);
    needles
        .iter()
        .filter(|n| !n.is_empty() && contains(&hay, &normalise_path(n.as_bytes())))
        .cloned()
        .collect()
}

/// Scan `bytes` for each needle; return the needles found (empty means clean). Pure.
#[must_use]
pub fn scan(bytes: &[u8], needles: &[String]) -> Vec<String> {
    needles
        .iter()
        .filter(|n| !n.is_empty() && contains(bytes, n.as_bytes()))
        .cloned()
        .collect()
}

/// Does `haystack` contain `needle` as a byte substring?
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The **licensing tokens** that `forge-licence` compiles into *every* premium binary and that
/// a shipped build must not carry as plain text (ADR 0063 item 2): the entitlement location
/// strings and the tier/licensing wire field names. All go through `forge_licence::obf!`, so a
/// `release-premium` build stores only their XORed bytes, while a `dev` build (obf! is identity
/// when `debug_assertions` is on) still contains them — that dev build is the guard's positive
/// control, so these must be **present in the control**. Distinctive strings only: no short,
/// common substring that could appear by chance in a large binary.
#[must_use]
pub fn required_licence_needles() -> Vec<String> {
    [
        "FORGE_ENTITLEMENT",
        "entitlement.json",
        "expires_ms",
        "over_100k",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// Further obf'd licensing tokens that only *some* premium binaries link (so they are checked
/// absent in the hardened build but not required present in the control): the premium editor's
/// stderr refusal marker. The verifier's `VerifyError` diagnostic sentences are deliberately not
/// listed — after WP-45 the editor and CLI map the error to a localisation key and never
/// `Display` it, so those strings are dead-code-eliminated from the shipped binaries (absent for
/// a better reason than obfuscation); they stay behind `obf!` in `forge-licence` for any binary
/// (e.g. the droplet server) that does log them.
#[must_use]
pub fn opportunistic_licence_needles() -> Vec<String> {
    ["premium feature unavailable"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// The committed **dev** public key's 32 bytes — the needle a shipped hardened binary must not
/// contain (ADR 0063 item, WP-45 problem 3). A build with no `FORGE_LICENCE_PUBKEY` embeds this
/// key, and its secret seed is committed, so anyone could mint entitlements it accepts; a
/// hardened binary that carries it would trust forged Team licences. The bytes come straight
/// from `forge-licence`, so the needle never drifts from what ships.
#[must_use]
pub fn dev_key_needle() -> Vec<u8> {
    forge_licence::DEV_PUBLIC_KEY.to_vec()
}

/// A fixed, valid, **non-production** Ed25519 public key the check injects as
/// `FORGE_LICENCE_PUBKEY` for the hardened build **when the operator has not injected one**, so
/// `cargo xtask premium-protections` is self-contained and green on any machine: the hardened
/// artefact then embeds a real non-dev key (as a shipped build does), the dev-key scan proves the
/// forgeable dev key is absent, and a positive presence check proves the build honoured the
/// injected key. It is the public half of a fixed non-development check seed (WP-45); its private
/// half has no more authority than the committed dev seed and it is **not** the production key — a
/// real shipped build injects the production key through the same variable (ADR 0063). It is
/// deliberately not the dev key, so the dev-key guard stays meaningful.
pub const CHECK_PUBLIC_KEY_HEX: &str =
    "d02bf12f800013b070f5e9a58a21c80290b0863aac91263c0bb34a02d1f0944d";

/// Parse a 64-character hex public key into its 32 bytes (`None` on a bad length or non-hex
/// character), for the "the hardened binary embeds the expected key" positive check.
#[must_use]
pub fn parse_key_hex(hex: &str) -> Option<Vec<u8>> {
    let bytes = hex.trim().as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let mut out = Vec::with_capacity(32);
    let mut i = 0;
    while i < 32 {
        out.push((nibble(bytes[i * 2])? << 4) | nibble(bytes[i * 2 + 1])?);
        i += 1;
    }
    Some(out)
}

// ---- Symbol / debug-info check (ADR 0063 item 1, WP-45 problem 5) --------------------------
//
// The criterion asks the shipped binary carry no debug symbols. On MSVC, rustc keeps debug
// info in a separate PDB and `debug = false` still leaves an RSDS debug-directory record in the
// exe that names the PDB by *bare filename* (no build path leaks); `strip = "symbols"` removes
// the COFF symbol table. So on PE the meaningful assertions are **no COFF symbol table** and
// **no `.debug*` sections**. On ELF (the ubuntu leg) a stripped release has **no `.symtab` and
// no `.debug*` sections**, which a dev build does have.
//
// rustc/MSVC emits no COFF symbol table even for a dev build, so a real dev-vs-hardened control
// is not available for that assertion on Windows; the positive control is therefore the
// synthetic unit tests below (a buffer whose header/sections carry a symbol table or a `.debug`
// section is flagged; a clean one is not), which prove the parser is not vacuous.

fn le_u16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn le_u64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8)
        .map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// Debug/symbol artefacts a *shipped* binary must not carry. `Ok(findings)` (empty = clean) for
/// a recognised PE or ELF; `Err` if the bytes are neither (the guard fails loudly rather than
/// passing an unrecognised file).
pub fn symbol_findings(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.starts_with(b"MZ") {
        pe_symbol_findings(bytes)
    } else if bytes.starts_with(&[0x7f, b'E', b'L', b'F']) {
        elf_symbol_findings(bytes)
    } else {
        Err(
            "premium-protections: the built file is neither PE nor ELF — cannot check for \
             debug symbols (ADR 0063)"
                .to_string(),
        )
    }
}

fn pe_symbol_findings(b: &[u8]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let e_lfanew = le_u32(b, 0x3c).ok_or("PE: truncated DOS header")? as usize;
    if b.get(e_lfanew..e_lfanew + 4) != Some(&[b'P', b'E', 0, 0]) {
        return Err("PE: no PE signature".to_string());
    }
    let fh = e_lfanew + 4; // IMAGE_FILE_HEADER
    let num_sections = le_u16(b, fh + 2).ok_or("PE: truncated file header")?;
    let ptr_symbols = le_u32(b, fh + 8).ok_or("PE: truncated file header")?;
    let num_symbols = le_u32(b, fh + 12).ok_or("PE: truncated file header")?;
    let opt_size = le_u16(b, fh + 16).ok_or("PE: truncated file header")? as usize;
    if num_symbols != 0 || ptr_symbols != 0 {
        out.push(format!(
            "a COFF symbol table ({num_symbols} symbols at 0x{ptr_symbols:x})"
        ));
    }
    // Section headers follow the optional header, 40 bytes each; name is 8 bytes at offset 0.
    let mut sh = fh + 20 + opt_size;
    for _ in 0..num_sections {
        let name_bytes = b.get(sh..sh + 8).ok_or("PE: truncated section header")?;
        let name = section_name(name_bytes);
        if name.starts_with(".debug") {
            out.push(format!("a `{name}` section"));
        }
        sh += 40;
    }
    Ok(out)
}

fn elf_symbol_findings(b: &[u8]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let class = *b.get(4).ok_or("ELF: truncated header")?; // 1 = 32-bit, 2 = 64-bit
    let (e_shoff, e_shentsize_off, e_shnum_off, e_shstrndx_off) = match class {
        2 => (
            le_u64(b, 0x28).ok_or("ELF64: truncated header")? as usize,
            0x3a,
            0x3c,
            0x3e,
        ),
        1 => (
            le_u32(b, 0x20).ok_or("ELF32: truncated header")? as usize,
            0x2e,
            0x30,
            0x32,
        ),
        other => return Err(format!("ELF: unknown class {other}")),
    };
    let shentsize = le_u16(b, e_shentsize_off).ok_or("ELF: truncated header")? as usize;
    let shnum = le_u16(b, e_shnum_off).ok_or("ELF: truncated header")? as usize;
    let shstrndx = le_u16(b, e_shstrndx_off).ok_or("ELF: truncated header")? as usize;
    if e_shoff == 0 || shnum == 0 {
        return Ok(out); // fully stripped: no section table at all.
    }
    // The section-header string table: sh_name(u32) sh_type(u32) ... sh_offset ... sh_size ...
    // For sh_offset/sh_size, the field offset differs by class.
    let (name_at, offset_at, size_at) = if class == 2 {
        (0, 0x18, 0x20)
    } else {
        (0, 0x10, 0x14)
    };
    let read_field = |hdr: usize, at: usize| -> Option<usize> {
        if class == 2 {
            le_u64(b, hdr + at).map(|v| v as usize)
        } else {
            le_u32(b, hdr + at).map(|v| v as usize)
        }
    };
    let strtab_hdr = e_shoff + shstrndx * shentsize;
    let strtab_off = read_field(strtab_hdr, offset_at).ok_or("ELF: bad shstrtab offset")?;
    let strtab_size = read_field(strtab_hdr, size_at).ok_or("ELF: bad shstrtab size")?;
    let strtab = b
        .get(strtab_off..strtab_off + strtab_size)
        .ok_or("ELF: shstrtab out of range")?;
    for i in 0..shnum {
        let hdr = e_shoff + i * shentsize;
        let name_idx = le_u32(b, hdr + name_at).ok_or("ELF: truncated section header")? as usize;
        let name = cstr_at(strtab, name_idx);
        if name == ".symtab" || name.starts_with(".debug") || name == ".stab" {
            out.push(format!("a `{name}` section"));
        }
    }
    Ok(out)
}

/// A PE section name: 8 bytes, NUL-padded (a leading `/` long-name reference is left as-is —
/// `.debug*` names fit in 8 bytes).
fn section_name(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// A NUL-terminated string at `idx` in an ELF string table.
fn cstr_at(strtab: &[u8], idx: usize) -> String {
    let rest = strtab.get(idx..).unwrap_or(&[]);
    let end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
    String::from_utf8_lossy(&rest[..end]).into_owned()
}

/// Check that `[profile.release-premium]` in `cargo_toml` sets every hardening key.
fn check_profile(cargo_toml: &str) -> Vec<String> {
    let Some(section) = profile_section(cargo_toml) else {
        return vec![format!(
            "root Cargo.toml has no [profile.{PROFILE}] section (ADR 0063)"
        )];
    };
    let mut errs = Vec::new();
    for (key, accepted) in required_profile_keys() {
        // The value token: the first whitespace-delimited word, without a trailing comment or
        // comma. Compared exactly, so `codegen-units = 16` does not satisfy an accepted `1`.
        match profile_value(section, key).map(value_token) {
            Some(tok) if accepted.contains(&tok) => {}
            Some(tok) => errs.push(format!(
                "[profile.{PROFILE}] {key} = {tok} — expected one of {accepted:?} (ADR 0063)"
            )),
            None => errs.push(format!(
                "[profile.{PROFILE}] is missing `{key}` (expected one of {accepted:?}, ADR 0063)"
            )),
        }
    }
    errs
}

/// The lines of the `[profile.release-premium]` section (up to the next `[`).
fn profile_section(cargo_toml: &str) -> Option<&str> {
    let header = format!("[profile.{PROFILE}]");
    let start = cargo_toml.find(&header)? + header.len();
    let rest = &cargo_toml[start..];
    let end = rest.find("\n[").map_or(rest.len(), |i| i + 1);
    Some(&rest[..end])
}

/// The comparable token of a profile value: the first whitespace-delimited word, without a
/// trailing comma or inline comment.
fn value_token(val: &str) -> &str {
    val.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(',')
}

/// The value text after `key =` in a profile section (the rest of that line, trimmed).
fn profile_value<'a>(section: &'a str, key: &str) -> Option<&'a str> {
    for line in section.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key)
            && let Some(v) = rest.trim_start().strip_prefix('=')
        {
            return Some(v.trim());
        }
    }
    None
}

/// The target directory (CARGO_TARGET_DIR, else `<root>/target`), absolute.
fn target_dir(root: &Path) -> PathBuf {
    let t = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    if t.is_absolute() { t } else { root.join(t) }
}

/// The built binary's path for a profile's output directory name (`debug` for dev, else the
/// profile name).
fn bin_path(root: &Path, out_dir: &str, bin: &str) -> PathBuf {
    let exe = if cfg!(windows) {
        format!("{bin}.exe")
    } else {
        bin.to_string()
    };
    target_dir(root).join(out_dir).join(exe)
}

/// The path prefixes the hardened build remaps, each with its neutral token, in the order rustc
/// must see them (pure). rustc applies the **last** matching `--remap-path-prefix`, so the list
/// runs from the least specific prefix to the most: the home directory (`~`), the workspace root
/// (`/forge`), the cargo target directory (`/target`, WP-47) and the hardened profile's
/// build-script output directory (`OUT_DIR`'s parent, `/target/build`) — a path under several
/// of them is remapped by the deepest. `drop_target` leaves the two target-directory remaps out:
/// the break test of the target-dir guard (the gate must then fail).
#[must_use]
pub fn remap_prefixes(
    root: &Path,
    target: &Path,
    homes: &[String],
    drop_target: bool,
) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = homes.iter().map(|h| (h.clone(), "~".into())).collect();
    v.push((root.display().to_string(), "/forge".into()));
    if !drop_target {
        v.push((target.display().to_string(), "/target".into()));
        v.push((
            target.join(PROFILE).join("build").display().to_string(),
            "/target/build".into(),
        ));
    }
    // Least specific first (a longer prefix of the same path is more specific).
    v.sort_by_key(|(from, _)| normalise_path(from.as_bytes()).len());
    v
}

/// The rustc flags for [`remap_prefixes`] (the stable equivalent of cargo's unstable
/// `trim-paths`), one argument each.
fn remap_flags(root: &Path, drop_target: bool) -> Vec<String> {
    remap_prefixes(root, &target_dir(root), &homes(), drop_target)
        .into_iter()
        .map(|(from, to)| format!("--remap-path-prefix={from}={to}"))
        .collect()
}

/// Build `bin` with `--profile profile` (or the `dev` profile when `profile` is `"dev"`). The
/// hardened profile additionally applies the path-remapping RUSTFLAGS.
///
/// The `dev` **control** build removes `FORGE_LICENCE_PUBKEY` from the environment so it embeds
/// the committed dev key and (being a `debug_assertions` build) the plain licensing strings —
/// exactly the artefacts the hardened build must not carry (`pubkey` is ignored for it).
///
/// The hardened build injects `pubkey` as `FORGE_LICENCE_PUBKEY` (`forge-licence`'s `key.rs`
/// reads it at compile time), so it embeds a **non-dev** public key exactly as a shipped build
/// does — the operator's production key when they set it, else the fixed check key
/// ([`CHECK_PUBLIC_KEY_HEX`]). The caller then proves the dev key is absent and this key is
/// present (WP-45 problem 3). Changing the value causes cargo to recompile `forge-licence`.
fn build(
    root: &Path,
    bin: &str,
    profile: &str,
    pubkey: Option<&str>,
    drop_target_remap: bool,
) -> XResult<()> {
    let mut cmd = Command::new("cargo");
    // `--bin <name>` builds that binary wherever it lives, so the output file is `<name>.exe`
    // and `bin` is both the build target and the scanned file's stem (the binary name may
    // differ from its package name).
    cmd.current_dir(root).arg("build").args(["--bin", bin]);
    if profile == "dev" {
        // `dev` is the default profile; `--profile dev` also works, output in `debug/`.
        cmd.args(["--profile", "dev"]);
        // The control must embed the dev key, whatever the ambient environment holds.
        cmd.env_remove("FORGE_LICENCE_PUBKEY");
    } else {
        cmd.args(["--profile", profile]);
        // Embed a non-dev public key, as a shipped build does (the operator's production key, or
        // the fixed check key). Without this the build would fall back to the forgeable dev key.
        if let Some(key) = pubkey {
            cmd.env("FORGE_LICENCE_PUBKEY", key);
        }
        // Remap build-machine paths out of the shipped binary (see remap_flags), after any
        // flags the caller set. Passed as CARGO_ENCODED_RUSTFLAGS (one argument per 0x1f-separated
        // field), so a path with a space stays one argument; cargo prefers it to RUSTFLAGS.
        let mut flags: Vec<String> = match std::env::var("CARGO_ENCODED_RUSTFLAGS") {
            Ok(s) if !s.is_empty() => s.split('\u{1f}').map(str::to_string).collect(),
            _ => std::env::var("RUSTFLAGS")
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        };
        flags.extend(remap_flags(root, drop_target_remap));
        cmd.env_remove("RUSTFLAGS");
        cmd.env("CARGO_ENCODED_RUSTFLAGS", flags.join("\u{1f}"));
    }
    let status = cmd.status().map_err(|e| {
        Failure::one(format!(
            "premium-protections: cargo build failed to run: {e}"
        ))
    })?;
    if !status.success() {
        return Err(Failure::one(format!(
            "premium-protections: `cargo build --bin {bin} --profile {profile}` failed"
        )));
    }
    Ok(())
}

/// How `premium-protections` runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// The break test (W2, WP-47): build the hardened binary **without** the target-directory
    /// remaps. The gate must then fail, naming the target directory's path left in the binary;
    /// a pass would mean the target-dir check is blind.
    pub drop_target_remap: bool,
}

/// Parse `premium-protections`' arguments: `[BIN] [--drop-target-remap]`.
#[must_use]
pub fn parse_args(args: &[String]) -> (Option<&str>, Options) {
    let mut bin = None;
    let mut o = Options::default();
    for a in args {
        match a.as_str() {
            "--drop-target-remap" => o.drop_target_remap = true,
            other => {
                if bin.is_none() {
                    bin = Some(other);
                }
            }
        }
    }
    (bin, o)
}

/// Run the guard for `bin` — a private-edition binary the caller names (this base tool holds
/// no premium name of its own; the name is supplied by the private CI invocation).
pub fn run(root: &Path, bin: Option<&str>, opts: Options) -> XResult<String> {
    let Some(bin) = bin else {
        return Err(Failure::one(
            "premium-protections needs the binary to build, e.g. \
             `cargo xtask premium-protections <bin>` (the private-edition binary name is \
             supplied by the caller, not this base tool)"
                .to_string(),
        ));
    };
    let mut out = String::new();

    // 1. The profile is hardened (a fast file check, always run).
    let cargo_toml = read(&root.join("Cargo.toml"))?;
    Failure::many(check_profile(&cargo_toml))?;
    out.push_str(&format!(
        "premium-protections: [profile.{PROFILE}] is hardened\n"
    ));

    let path_needles = build_path_needles(root);
    let target_path_needles = target_needles(root, &target_dir(root), &homes());
    let required_strings = required_licence_needles();
    // Every licensing token that must be absent from the hardened binary (required ∪ opportunistic).
    let mut all_strings = required_strings.clone();
    all_strings.extend(opportunistic_licence_needles());
    let dev_key = dev_key_needle();

    // The non-dev public key the hardened build embeds: the operator's production key when they
    // set FORGE_LICENCE_PUBKEY, else the fixed check key so the check is self-contained (WP-45
    // problem 3). Its bytes are the "the build honoured the key" positive needle below.
    let operator_key = std::env::var("FORGE_LICENCE_PUBKEY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let embedded_hex = operator_key
        .clone()
        .unwrap_or_else(|| CHECK_PUBLIC_KEY_HEX.to_string());
    let embedded_key = parse_key_hex(&embedded_hex).ok_or_else(|| {
        Failure::one(format!(
            "premium-protections: FORGE_LICENCE_PUBKEY is not 64 hex characters: {embedded_hex:?}"
        ))
    })?;

    // 2. The positive control: an ordinary dev build of the same binary (no FORGE_LICENCE_PUBKEY)
    // DOES contain the build-machine paths, the plain licensing strings and the dev key — so
    // each clean-binary check below is provably non-vacuous.
    build(root, bin, "dev", None, false)?;
    let dev_bin = bin_path(root, "debug", bin);
    let dev_bytes = std::fs::read(&dev_bin).map_err(|e| {
        Failure::one(format!(
            "premium-protections: read {}: {e}",
            dev_bin.display()
        ))
    })?;
    let mut control_errs = Vec::new();
    let ctl_paths = scan_paths(&dev_bytes, &path_needles);
    if ctl_paths.is_empty() {
        control_errs.push(format!(
            "the dev build of {bin} contains none of the build-machine needles {path_needles:?} \
             — the clean-path check would prove nothing"
        ));
    }
    // WP-47: the target directory in particular (build scripts' OUT_DIR paths), so the
    // target-dir check below is not vacuous either.
    if scan_paths(&dev_bytes, &target_path_needles).is_empty() {
        control_errs.push(format!(
            "the dev build of {bin} contains no target-directory path {target_path_needles:?} — \
             the target-dir check would prove nothing"
        ));
    }
    let ctl_strings = scan(&dev_bytes, &required_strings);
    let missing_ctl_strings: Vec<&String> = required_strings
        .iter()
        .filter(|n| !ctl_strings.contains(n))
        .collect();
    if !missing_ctl_strings.is_empty() {
        control_errs.push(format!(
            "the dev build of {bin} is missing licensing strings {missing_ctl_strings:?} — the \
             obfuscation check would prove nothing (are they built into this binary at all?)"
        ));
    }
    if !contains(&dev_bytes, &dev_key) {
        control_errs.push(format!(
            "the dev build of {bin} does not embed the dev public key — the dev-key check would \
             prove nothing (was FORGE_LICENCE_PUBKEY leaked into the control build?)"
        ));
    }
    Failure::many(control_errs)?;
    out.push_str(&format!(
        "premium-protections: control OK — the dev build of {bin} contains build-machine paths \
         (the target directory's among them), the {} required licensing strings in plain text, \
         and the dev public key\n",
        required_strings.len()
    ));

    // 3. The hardened binary is clean on every count.
    if opts.drop_target_remap {
        out.push_str(
            "premium-protections: BREAK TEST — building without the target-directory remaps; \
             the gate must fail below\n",
        );
    }
    out.push_str(&format!(
        "premium-protections: the hardened build remaps {:?}\n",
        remap_flags(root, opts.drop_target_remap)
    ));
    build(
        root,
        bin,
        PROFILE,
        Some(&embedded_hex),
        opts.drop_target_remap,
    )?;
    let hardened = bin_path(root, PROFILE, bin);
    let bytes = std::fs::read(&hardened).map_err(|e| {
        Failure::one(format!(
            "premium-protections: read {}: {e}",
            hardened.display()
        ))
    })?;
    let mut errs = Vec::new();

    let found_paths = scan_paths(&bytes, &path_needles);
    if !found_paths.is_empty() {
        errs.push(format!(
            "the hardened binary contains build-machine paths: {found_paths:?} — trim-paths \
             (--remap-path-prefix) is not taking effect"
        ));
    }
    let found_strings = scan(&bytes, &all_strings);
    if !found_strings.is_empty() {
        errs.push(format!(
            "the hardened binary contains licensing strings in plain text: {found_strings:?} — \
             they are not going through forge_licence::obf! (ADR 0063 item 2)"
        ));
    }
    if contains(&bytes, &dev_key) {
        errs.push(
            "the hardened binary embeds the committed DEV public key — set FORGE_LICENCE_PUBKEY \
             to the production key for a shipped build (the dev seed is committed, so the dev key \
             accepts entitlements anyone can mint; ADR 0063, WP-45)"
                .to_string(),
        );
    }
    // The build must have honoured FORGE_LICENCE_PUBKEY: the non-dev key we injected (or the
    // operator's production key) is actually embedded. If it is absent, `key.rs` did not pick the
    // injected key up and the dev-key-absent check above would be passing vacuously.
    if !contains(&bytes, &embedded_key) {
        errs.push(format!(
            "the hardened binary does not embed the expected public key ({embedded_hex}) — \
             FORGE_LICENCE_PUBKEY was not compiled in, so the dev-key check is not meaningful \
             (ADR 0063, WP-45)"
        ));
    }
    match symbol_findings(&bytes) {
        Ok(findings) if !findings.is_empty() => errs.push(format!(
            "the hardened binary carries debug symbols: {findings:?} — expected none (strip = \
             \"symbols\", debug = false; ADR 0063 item 1)"
        )),
        Ok(_) => {}
        Err(e) => errs.push(e),
    }
    if !errs.is_empty() {
        // What passed before the failure (the profile, the control) stays in the record.
        print!("{out}");
    }
    Failure::many(
        errs.into_iter()
            .map(|e| format!("premium-protections: {}: {e}", hardened.display()))
            .collect(),
    )?;
    let key_note = if operator_key.is_some() {
        "the operator's FORGE_LICENCE_PUBKEY"
    } else {
        "the fixed non-production check key (set FORGE_LICENCE_PUBKEY to ship)"
    };
    out.push_str(&format!(
        "premium-protections: {} is clean — no build-machine paths, no plain-text licensing \
         strings, no dev key, no debug symbols, and it embeds {key_note} (scanned {} bytes)\n",
        hardened.display(),
        bytes.len()
    ));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_a_planted_path_and_passes_a_clean_buffer() {
        let needles = vec!["C:\\Users\\dev\\Forge".to_string()];
        // Positive control: a buffer holding the path is caught.
        let dirty = b"...some bytes C:\\Users\\dev\\Forge\\src\\main.rs more...".to_vec();
        assert_eq!(scan(&dirty, &needles), needles);
        // A clean buffer (paths trimmed) is not.
        let clean = b"...some bytes src/main.rs more...".to_vec();
        assert!(scan(&clean, &needles).is_empty());
    }

    /// A synthetic machine: a workspace under the home directory and a lane target directory
    /// beside it (the layout WP-45's verifier found leaking).
    fn machine() -> (PathBuf, PathBuf, Vec<String>) {
        (
            PathBuf::from("C:\\Users\\dev\\Documents\\GitHub\\Forge"),
            PathBuf::from("C:\\Users\\dev\\ForgeTargets\\B"),
            vec!["C:\\Users\\dev".to_string()],
        )
    }

    /// WP-47 (WP-45 follow-up b): the positive control that plants a target-dir path. A binary
    /// whose build scripts' output paths were remapped only by the home directory still names
    /// the lane's target directory (`~\ForgeTargets/B\release-premium\build\...`, mixed
    /// separators as rustc writes them); the scan must catch it, and the fully remapped form
    /// (`/target/build\...`) must pass.
    #[test]
    fn a_target_dir_path_left_by_a_home_only_remap_is_caught() {
        let (root, target, homes) = machine();
        let needles = path_needles(&root, &target, &homes);
        let planted = b"..internal error: entered unreachable code: isle_x64.isle line 2247; \
                        ~\\ForgeTargets/B\\release-premium\\build\\cranelift-codegen-9e1c\\out/isle_x64.rs.."
            .to_vec();
        let found = scan_paths(&planted, &needles);
        assert!(
            found.iter().any(|n| n.contains("ForgeTargets")),
            "a home-remapped target dir was not caught: {found:?} (needles {needles:?})"
        );
        // The absolute form is caught too, whatever the case and the separators.
        let absolute = b"..c:/users/dev/forgetargets/b/release-premium/build/x/out/a.rs..".to_vec();
        assert!(!scan_paths(&absolute, &needles).is_empty());
        // With the target-dir remap in force the path names nothing of this machine.
        let clean = b"..isle_x64.isle line 2247; /target/build\\cranelift-codegen-9e1c\\out/isle_x64.rs \
                      src/main.rs ~\\.cargo\\registry\\src\\index.crates.io-1949cf8c6b5b557f\\x.rs.."
            .to_vec();
        assert!(
            scan_paths(&clean, &needles).is_empty(),
            "{:?}",
            scan_paths(&clean, &needles)
        );
        // The target needles alone (the dev control's requirement) hold the target dir.
        let t = target_needles(&root, &target, &homes);
        assert!(t.iter().any(|n| n.starts_with('~')), "{t:?}");
        assert!(!scan_paths(&planted, &t).is_empty());
    }

    /// The remaps run from the least specific prefix to the deepest (rustc applies the last
    /// that matches): a build-script output path is remapped by the target-dir entries, not
    /// by the home directory's; the break test leaves those entries out.
    #[test]
    fn the_deepest_remap_comes_last_and_the_break_test_drops_the_target_ones() {
        let (root, target, homes) = machine();
        let full = remap_prefixes(&root, &target, &homes, false);
        let tokens: Vec<&str> = full.iter().map(|(_, to)| to.as_str()).collect();
        assert_eq!(tokens.first(), Some(&"~"), "{full:?}");
        let pos = |t: &str| tokens.iter().position(|x| *x == t).unwrap_or(usize::MAX);
        assert!(pos("~") < pos("/target") && pos("/target") < pos("/target/build"));
        // The last prefix that matches an OUT_DIR path is the build-output one.
        let out_dir = "C:\\Users\\dev\\ForgeTargets\\B\\release-premium\\build\\cc-1\\out";
        let last = full
            .iter()
            .rev()
            .find(|(from, _)| strip_path_prefix(out_dir, from).is_some())
            .map(|(_, to)| to.as_str());
        assert_eq!(last, Some("/target/build"));
        let broken = remap_prefixes(&root, &target, &homes, true);
        assert!(
            broken.iter().all(|(_, to)| !to.starts_with("/target")),
            "{broken:?}"
        );
        let last = broken
            .iter()
            .rev()
            .find(|(from, _)| strip_path_prefix(out_dir, from).is_some())
            .map(|(_, to)| to.as_str());
        assert_eq!(
            last,
            Some("~"),
            "without the target remaps the home one applies"
        );
    }

    #[test]
    fn prefixes_strip_by_component() {
        assert_eq!(strip_path_prefix("C:\\a\\bc", "C:\\a"), Some("\\bc"));
        assert_eq!(strip_path_prefix("C:/A/bc", "c:\\a\\"), Some("/bc"));
        assert_eq!(strip_path_prefix("C:\\ab", "C:\\a"), None);
        assert_eq!(strip_path_prefix("C:\\a", "C:\\a"), None);
    }

    #[test]
    fn the_break_flag_parses() {
        let args = vec!["bin".to_string(), "--drop-target-remap".to_string()];
        assert_eq!(
            parse_args(&args),
            (
                Some("bin"),
                Options {
                    drop_target_remap: true
                }
            )
        );
        assert_eq!(parse_args(&[]), (None, Options::default()));
    }

    #[test]
    fn contains_matches_substrings_and_rejects_the_absent() {
        assert!(contains(b"abcdef", b"cde"));
        assert!(contains(b"abc", b"abc"));
        assert!(!contains(b"abc", b"abcd"));
        assert!(!contains(b"abc", b""));
    }

    #[test]
    fn a_hardened_profile_passes_and_a_slack_one_fails() {
        let good = "\
[profile.release-premium]
inherits = \"release\"
strip = \"symbols\"
debug = false
lto = \"fat\"
codegen-units = 1
overflow-checks = false
panic = \"unwind\"
";
        assert!(check_profile(good).is_empty(), "{:?}", check_profile(good));

        // Positive control: a profile with debug on and thin LTO fails, naming both.
        let slack = "\
[profile.release-premium]
inherits = \"release\"
strip = \"symbols\"
debug = true
lto = \"thin\"
codegen-units = 16
";
        let errs = check_profile(slack);
        assert!(errs.iter().any(|e| e.contains("debug")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("lto")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("codegen-units")), "{errs:?}");

        // No section at all fails.
        assert!(!check_profile("[profile.release]\nlto = \"thin\"\n").is_empty());
    }

    #[test]
    fn profile_value_reads_the_line() {
        let section = "\ninherits = \"release\"\ncodegen-units = 1\n";
        assert_eq!(profile_value(section, "inherits"), Some("\"release\""));
        assert_eq!(profile_value(section, "codegen-units"), Some("1"));
        assert_eq!(profile_value(section, "lto"), None);
    }

    #[test]
    fn licence_strings_are_found_in_plain_text_and_not_when_absent() {
        let mut needles = required_licence_needles();
        needles.extend(opportunistic_licence_needles());
        assert!(needles.len() >= 4);
        // Positive control: a buffer holding a token is caught.
        let dirty =
            b"..noise..FORGE_ENTITLEMENT..over_100k..premium feature unavailable..".to_vec();
        let found = scan(&dirty, &needles);
        assert!(found.iter().any(|n| n == "FORGE_ENTITLEMENT"), "{found:?}");
        assert!(found.iter().any(|n| n == "over_100k"), "{found:?}");
        assert!(
            found.iter().any(|n| n == "premium feature unavailable"),
            "{found:?}"
        );
        // Obfuscated bytes (here just other noise) are not.
        assert!(scan(b"..only noise here..", &needles).is_empty());
    }

    #[test]
    fn the_check_key_is_valid_non_dev_and_parses() {
        let key = parse_key_hex(CHECK_PUBLIC_KEY_HEX).expect("check key is 64 hex chars");
        assert_eq!(key.len(), 32);
        // It must not be the dev key, or injecting it would defeat the dev-key guard.
        assert_ne!(
            key,
            dev_key_needle(),
            "the check key must differ from the dev key"
        );
        // Bad inputs are rejected (not silently truncated).
        assert!(parse_key_hex("abcd").is_none(), "short hex is rejected");
        assert!(
            parse_key_hex(&"zz".repeat(32)).is_none(),
            "non-hex is rejected"
        );
        assert!(
            parse_key_hex(&format!("{CHECK_PUBLIC_KEY_HEX}00")).is_none(),
            "over-long hex is rejected"
        );
    }

    #[test]
    fn the_expected_key_presence_check_is_non_vacuous() {
        // The positive check the hardened scan runs: the injected key's bytes must be present.
        let key = parse_key_hex(CHECK_PUBLIC_KEY_HEX).unwrap();
        let mut with = vec![0u8; 4];
        with.extend_from_slice(&key);
        with.extend_from_slice(&[0u8; 4]);
        assert!(contains(&with, &key), "the key is found when embedded");
        // A binary that fell back to the dev key does NOT contain the injected key.
        assert!(
            !contains(&dev_key_needle(), &key),
            "a build that ignored the injected key must fail the presence check"
        );
    }

    #[test]
    fn the_dev_key_needle_is_found_only_when_present() {
        let key = dev_key_needle();
        assert_eq!(key.len(), 32);
        let mut dirty = vec![0u8; 8];
        dirty.extend_from_slice(&key);
        dirty.extend_from_slice(&[0u8; 8]);
        assert!(
            contains(&dirty, &key),
            "the dev key must be found when embedded"
        );
        // A binary with a different (production) key does not contain the dev key.
        let mut other = key.clone();
        other[0] ^= 0xff;
        let mut clean = vec![0u8; 8];
        clean.extend_from_slice(&other);
        assert!(
            !contains(&clean, &key),
            "a non-dev key must not match the dev needle"
        );
    }

    /// A minimal PE with a given COFF symbol count and one section name.
    fn synthetic_pe(num_symbols: u32, section: &str) -> Vec<u8> {
        let mut b = vec![0u8; 0x100];
        b[0] = b'M';
        b[1] = b'Z';
        let e_lfanew: u32 = 0x40;
        b[0x3c..0x40].copy_from_slice(&e_lfanew.to_le_bytes());
        let pe = e_lfanew as usize;
        b[pe..pe + 4].copy_from_slice(&[b'P', b'E', 0, 0]);
        let fh = pe + 4;
        b[fh + 2..fh + 4].copy_from_slice(&1u16.to_le_bytes()); // NumberOfSections
        let ptr = if num_symbols == 0 { 0u32 } else { 0x1000 };
        b[fh + 8..fh + 12].copy_from_slice(&ptr.to_le_bytes()); // PointerToSymbolTable
        b[fh + 12..fh + 16].copy_from_slice(&num_symbols.to_le_bytes()); // NumberOfSymbols
        b[fh + 16..fh + 18].copy_from_slice(&0u16.to_le_bytes()); // SizeOfOptionalHeader
        let sh = fh + 20;
        let name = section.as_bytes();
        b[sh..sh + name.len().min(8)].copy_from_slice(&name[..name.len().min(8)]);
        b
    }

    #[test]
    fn pe_symbol_check_flags_a_symbol_table_and_a_debug_section() {
        // Clean: no symbols, ordinary section.
        assert!(
            symbol_findings(&synthetic_pe(0, ".text"))
                .unwrap()
                .is_empty()
        );
        // Positive control: a COFF symbol table is flagged.
        let with_syms = symbol_findings(&synthetic_pe(5, ".text")).unwrap();
        assert!(
            with_syms.iter().any(|f| f.contains("symbol table")),
            "{with_syms:?}"
        );
        // Positive control: a .debug section is flagged.
        let with_dbg = symbol_findings(&synthetic_pe(0, ".debug$S")).unwrap();
        assert!(
            with_dbg.iter().any(|f| f.contains(".debug")),
            "{with_dbg:?}"
        );
    }

    /// A minimal ELF64 with three sections (null, shstrtab, and one named `third`).
    fn synthetic_elf(third: &str) -> Vec<u8> {
        let shstr = format!("\0.shstrtab\0{third}\0");
        let third_idx = 1 + ".shstrtab".len() + 1; // after "\0.shstrtab\0"
        let e_shoff: u64 = 64;
        let shentsize: u16 = 64;
        let shnum: u16 = 3;
        let shstrndx: u16 = 1;
        let strtab_off = 64 + (shnum as usize) * (shentsize as usize);
        let total = strtab_off + shstr.len();
        let mut b = vec![0u8; total];
        b[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        b[4] = 2; // 64-bit
        b[0x28..0x30].copy_from_slice(&e_shoff.to_le_bytes());
        b[0x3a..0x3c].copy_from_slice(&shentsize.to_le_bytes());
        b[0x3c..0x3e].copy_from_slice(&shnum.to_le_bytes());
        b[0x3e..0x40].copy_from_slice(&shstrndx.to_le_bytes());
        let hdr = |i: usize| 64 + i * 64;
        // Section 1 = shstrtab: name index 1 (".shstrtab"), offset + size of the string table.
        b[hdr(1)..hdr(1) + 4].copy_from_slice(&1u32.to_le_bytes());
        b[hdr(1) + 0x18..hdr(1) + 0x20].copy_from_slice(&(strtab_off as u64).to_le_bytes());
        b[hdr(1) + 0x20..hdr(1) + 0x28].copy_from_slice(&(shstr.len() as u64).to_le_bytes());
        // Section 2 = `third`.
        b[hdr(2)..hdr(2) + 4].copy_from_slice(&(third_idx as u32).to_le_bytes());
        b[strtab_off..].copy_from_slice(shstr.as_bytes());
        b
    }

    #[test]
    fn elf_symbol_check_flags_symtab_and_debug_sections() {
        // Clean: ordinary section names only.
        assert!(symbol_findings(&synthetic_elf(".text")).unwrap().is_empty());
        // Positive control: a .symtab section is flagged.
        let with_symtab = symbol_findings(&synthetic_elf(".symtab")).unwrap();
        assert!(
            with_symtab.iter().any(|f| f.contains(".symtab")),
            "{with_symtab:?}"
        );
        // Positive control: a .debug_info section is flagged.
        let with_dbg = symbol_findings(&synthetic_elf(".debug_info")).unwrap();
        assert!(
            with_dbg.iter().any(|f| f.contains(".debug_info")),
            "{with_dbg:?}"
        );
    }

    #[test]
    fn an_unrecognised_binary_format_is_an_error_not_a_pass() {
        assert!(symbol_findings(b"not a binary at all").is_err());
    }
}
