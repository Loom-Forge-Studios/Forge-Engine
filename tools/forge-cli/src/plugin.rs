//! `forge plugin check` — load a WASM plugin directory the way the engine does, and report.
//!
//! ```text
//! forge plugin check <dir> [--grant CAP]... [--run TARGET [ARGS_JSON]]
//! ```
//!
//! The directory holds `plugin.ron` and `plugin.wasm` (or `plugin.wat`). The plugin is
//! compiled by the WASM host (`forge-wasm`), its imports checked against its manifest, and
//! installed through the ordinary loader (`loader::load_hosted`) next to the first-party
//! presets, onto the editor's extension points — the same path the editor takes. The report
//! lists every item it installed (and what it replaced or chained), and what capabilities it
//! requests against what `--grant` granted. `--run` then applies one of its commands on a
//! fresh project as a human would, and prints what changed: a plugin author's loop without an
//! editor. Nothing is granted unless `--grant` says so (`Command(Ordinary)`,
//! `Fs(ProjectRead)`, …; the manifest spelling).

use std::path::PathBuf;

use forge_cmd::{Bus, CommandSink, EditorCommand, Issuer};
use forge_plugin::points::{Command, install_commands};
use forge_plugin::{Capability, Principal, SharedGrants, loader};
use forge_wasm::WasmHost;

use crate::CliError;

/// Parsed `forge plugin check` arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckArgs {
    /// The plugin directory.
    pub dir: PathBuf,
    /// Capabilities to grant the plugin.
    pub grants: Vec<Capability>,
    /// A command target to run, with its JSON arguments.
    pub run: Option<(String, String)>,
}

/// Parse the arguments after `forge plugin`.
pub fn parse(args: &[String]) -> Result<CheckArgs, CliError> {
    let usage = |m: &str| CliError::Usage(m.to_string());
    match args.first().map(String::as_str) {
        Some("check") => {}
        Some(other) => return Err(usage(&format!("unknown plugin command `{other}`"))),
        None => return Err(usage("forge plugin needs a command (check)")),
    }
    let mut dir = None;
    let mut grants = Vec::new();
    let mut run = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--grant" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| usage("--grant needs a capability"))?;
                let cap: Capability = ron::from_str(v).map_err(|_| {
                    usage(&format!(
                        "--grant {v:?} is not a capability (Fs(ProjectRead), Command(Ordinary), ...)"
                    ))
                })?;
                grants.push(cap);
                i += 2;
            }
            "--run" => {
                let target = args
                    .get(i + 1)
                    .ok_or_else(|| usage("--run needs a command target"))?;
                let (json, used) = match args.get(i + 2) {
                    Some(a) if !a.starts_with("--") => (a.clone(), 3),
                    _ => ("{}".to_string(), 2),
                };
                run = Some((target.clone(), json));
                i += used;
            }
            flag if flag.starts_with("--") => {
                return Err(usage(&format!("unknown flag `{flag}`")));
            }
            path => {
                if dir.replace(PathBuf::from(path)).is_some() {
                    return Err(usage("forge plugin check takes one directory"));
                }
                i += 1;
            }
        }
    }
    Ok(CheckArgs {
        dir: dir.ok_or_else(|| usage("forge plugin check needs a plugin directory"))?,
        grants,
        run,
    })
}

/// Run `forge plugin check`; the report goes to `out`.
pub fn check(a: &CheckArgs, out: &mut dyn std::io::Write) -> Result<(), CliError> {
    let io = |e: std::io::Error| CliError::step("report", e);
    let grants = SharedGrants::new();
    let host = WasmHost::new(grants.clone()).map_err(|e| CliError::step("host", e))?;
    let plugin = host
        .load_dir(&a.dir)
        .map_err(|e| CliError::step("load", e))?;
    let who = Principal::Plugin(plugin.id().clone());
    for cap in &a.grants {
        grants.grant(who.clone(), *cap);
    }

    let mut ext = forge_editor::editor_extensions().map_err(|e| CliError::step("points", e))?;
    let presets = forge_presets::BuiltinPresets::new().map_err(|e| CliError::step("points", e))?;
    let report = loader::load_hosted(&mut ext, &[&presets], &[&plugin], &[], &grants.snapshot())
        .map_err(|e| CliError::step("install", e))?;

    let m = forge_plugin::HostedPlugin::manifest(&plugin);
    writeln!(out, "plugin {} {} (WASM)", m.id, m.version).map_err(io)?;
    for (point, key, prov) in ext.inventory() {
        let mine = prov.owner == m.id
            || prov.replaced_by.as_ref() == Some(&m.id)
            || prov.chained_by.contains(&m.id);
        if !mine {
            continue;
        }
        let how = if prov.owner == m.id {
            "provides"
        } else if prov.replaced_by.as_ref() == Some(&m.id) {
            "replaces"
        } else {
            "chains"
        };
        writeln!(out, "  {how} {}({key:?})", point.name).map_err(io)?;
    }
    for r in &m.removes {
        writeln!(out, "  removes {r}").map_err(io)?;
    }
    for c in report.capabilities.iter().filter(|c| c.plugin == m.id) {
        for cap in &c.requested {
            let state = if c.granted.contains(cap) {
                "granted"
            } else {
                "NOT granted"
            };
            writeln!(out, "  capability {cap}: {state}").map_err(io)?;
        }
    }

    if let Some((target, json)) = &a.run {
        let mut bus = Bus::new();
        let reg = ext
            .registry::<Command>()
            .ok_or_else(|| CliError::check("run", "the Command point is not defined"))?;
        install_commands(reg, &mut bus).map_err(|e| CliError::step("run", e))?;
        let e = bus.envelope(
            Issuer::Human {
                user: "forge-plugin-check".into(),
            },
            EditorCommand::Invoke {
                target: target.clone(),
                args: json.clone(),
            },
        );
        let applied = bus.apply(e).map_err(|r| CliError::step("run", r.error))?;
        writeln!(
            out,
            "ran {target}: {} change(s), undoable",
            applied.diff.len()
        )
        .map_err(io)?;
        for (k, v) in bus.project().settings() {
            writeln!(out, "  setting {k} = {v:?}").map_err(io)?;
        }
        for (key, e) in bus.project().entities() {
            writeln!(out, "  entity {} {:?}", key.0, e.name()).map_err(io)?;
            for (path, v) in e.properties() {
                writeln!(out, "    {path} = {v:?}").map_err(io)?;
            }
        }
    }
    for e in host.drain_events() {
        writeln!(out, "  host: {e:?}").map_err(io)?;
    }
    Ok(())
}
