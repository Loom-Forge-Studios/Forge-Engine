//! Shared fixtures: WAT plugins, a command point on a bus, grants.

#![allow(dead_code)]

use forge_cmd::{Bus, CommandSink, EditorCommand, Issuer, Rejection};
use forge_plugin::points::{Command, install_commands};
use forge_plugin::{
    Capability, CommandClass, Extensions, HostedPlugin, Manifest, PluginError, Principal,
    SharedGrants, SourcePlugin, loader,
};
use forge_wasm::wat;

pub const ORDINARY: Capability = Capability::Command(CommandClass::Ordinary);
pub const DESTRUCTIVE: Capability = Capability::Command(CommandClass::Destructive);

pub fn manifest(text: &str) -> Manifest {
    Manifest::parse(text).unwrap_or_else(|e| panic!("{e}"))
}

/// A WASM command plugin manifest: `id` provides `Command(target)`.
pub fn command_manifest(id: &str, target: &str, caps: &str) -> Manifest {
    manifest(&format!(
        r#"Plugin(id: "{id}", version: "0.1.0", engine: "^0.1", kind: Wasm,
            provides: [Command("{target}")], capabilities: [{caps}])"#
    ))
}

/// A plugin that answers every call with `plan` (JSON of a command plan).
pub fn plan_plugin(plan: &str) -> String {
    wat::constant(plan.as_bytes())
}

pub fn who(m: &str) -> Principal {
    Principal::Plugin(forge_plugin::PluginId::new(m).unwrap_or_else(|e| panic!("{e}")))
}

/// Load plugins through the public loader into a fresh Command registry and a bus.
pub fn bus_with(
    sources: &[&dyn SourcePlugin],
    hosted: &[&dyn HostedPlugin],
    grants: &SharedGrants,
) -> Result<(Bus, Extensions), PluginError> {
    let mut ext = Extensions::new();
    ext.define::<Command>()?;
    loader::load_hosted(&mut ext, sources, hosted, &[], &grants.snapshot())?;
    let mut bus = Bus::new();
    let reg = ext
        .registry::<Command>()
        .unwrap_or_else(|| panic!("Command point defined"));
    install_commands(reg, &mut bus).unwrap_or_else(|e| panic!("{e}"));
    Ok((bus, ext))
}

pub fn run(bus: &mut Bus, target: &str, args: &str) -> Result<forge_cmd::TxnId, Rejection> {
    let e = bus.envelope(
        Issuer::Human { user: "ada".into() },
        EditorCommand::Invoke {
            target: target.into(),
            args: args.into(),
        },
    );
    let txn = e.txn;
    bus.apply(e).map(|_| txn)
}
