//! The `forge` command line (Ch.30, Ch.34.4): what the headless engine binary runs, in the
//! library so an edition's binary runs the same commands and adds its own ([`Subcommand`]).
//!
//! ```text
//! forge --headless [--script FILE|-] [--record FILE] [--replay FILE] [--trace FILE] [--accept-project-security] [--trust-project]
//! forge --headless --remote-host [--port N] [--lan] [--config-dir DIR | --no-user-config] [--script FILE]
//! forge plugin check <dir> [--grant CAP]... [--run TARGET [ARGS_JSON]]
//! ```
//!
//! `forge --headless` (Ch.34.4) runs the editor's core with no UI at all: editor commands and
//! play controls from a script (stdin when neither `--script` nor `--replay` is given), the
//! play session recorded to a replay file, a replay file replayed and checked bit-for-bit, a
//! Perfetto trace of the run. It is `forge_editor::headless`, the code `forge-editor
//! --headless` runs, so the GUI and the headless core are one engine.
//!
//! `forge --headless --remote-host` is the split editor's headless core (Ch.34, M2-16): it serves
//! the core to paired devices over QUIC (`forge_remote::console`) with no window or GPU, and
//! stdin becomes the host console where a person shows a pairing code and allows a device.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::CliError;

/// The base command line's usage (an edition's [`Subcommand`]s add theirs after it).
pub const USAGE: &str =
    "usage: forge --headless [--script FILE|-] [--record FILE] [--replay FILE] [--trace FILE] [--accept-project-security] [--trust-project]
       forge --headless --remote-host [--port N] [--lan] [--config-dir DIR | --no-user-config] [--script FILE] [--trust-project]
       forge plugin check <dir> [--grant CAP]... [--run TARGET [ARGS_JSON]]

  --headless  the editor core with no UI: editor commands and play controls from
          --script (or stdin), --record the play session to a replay file, --replay a
          file and check it bit-for-bit, --trace the run to a Perfetto file;
          --accept-project-security: plugin grants and the default automation policy a
          project the script opens carries take effect (a script's open otherwise
          holds them for a person, and a save keeps them on disk as they are);
          --trust-project: trust the projects the run opens (headless has no one to
          ask, so otherwise no project is trusted and the plugin grants a project
          carries stay held; with --remote-host the person at the console answers
          instead: `trust`, `trust ID`, `distrust ID`)
          --remote-host: also serve the core to paired devices (the split editor) over
          QUIC, on loopback unless --lan; stdin is then the host console, where a person
          shows pairing codes and allows devices (`pair`, `allow NAME`, `help`) and
          answers project trust (`trust`)

  plugin check  load a WASM plugin directory (plugin.ron + plugin.wasm or .wat) through
          the WASM host and the ordinary loader and report what it installs and which
          capabilities it requests; --grant CAP grants one (Command(Ordinary), ...), --run
          applies one of its commands on a fresh project and prints the result";

/// A command an edition's binary adds to the base ones (`forge <name> ...`). The base
/// binary adds none.
pub trait Subcommand {
    /// Its name: the first argument that selects it.
    fn name(&self) -> &'static str;
    /// Its usage line and its help paragraph, appended to [`USAGE`].
    fn usage(&self) -> &'static str;
    /// Run it with the arguments after its name.
    fn run(&self, args: &[String]) -> Result<(), CliError>;
}

/// The whole usage text: [`USAGE`] and every extra command's.
#[must_use]
pub fn usage(extra: &[&dyn Subcommand]) -> String {
    let mut s = USAGE.to_string();
    for x in extra {
        s.push_str("\n\n");
        s.push_str(x.usage());
    }
    s
}

/// `forge`'s `main`: the process arguments, the base commands and `extra`; errors go to
/// stderr (with the usage for a usage error) and the exit code.
pub fn main_with(extra: &[&dyn Subcommand]) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args, extra) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("forge: {e}");
            if matches!(e, CliError::Usage(_)) {
                eprintln!("{}", usage(extra));
            }
            ExitCode::FAILURE
        }
    }
}

/// Run one command line (`args` without the program name).
pub fn run(args: &[String], extra: &[&dyn Subcommand]) -> Result<(), CliError> {
    match args.first().map(String::as_str) {
        Some("--headless") => headless_cmd(&args[1..]),
        Some("plugin") => {
            crate::plugin::check(&crate::plugin::parse(&args[1..])?, &mut std::io::stdout())
        }
        Some("--help" | "-h" | "help") => {
            println!("{}", usage(extra));
            Ok(())
        }
        Some(other) => match extra.iter().find(|x| x.name() == other) {
            Some(x) => x.run(&args[1..]),
            None => Err(CliError::Usage(format!("unknown command `{other}`"))),
        },
        None => Err(CliError::Usage("no command given".into())),
    }
}

fn headless_cmd(args: &[String]) -> Result<(), CliError> {
    use forge_editor::headless::{HeadlessOptions, run};
    let mut script: Option<String> = None;
    let mut opts = HeadlessOptions::default();
    let mut remote_host = false;
    let mut host = forge_remote::console::ConsoleOptions::default();
    let mut config_dir: Option<PathBuf> = None;
    let mut no_user_config = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| CliError::Usage(format!("{flag} needs a value")))
        };
        match a.as_str() {
            "--script" => script = Some(value("--script")?),
            "--record" => opts.record = Some(PathBuf::from(value("--record")?)),
            "--replay" => opts.replay = Some(PathBuf::from(value("--replay")?)),
            "--trace" => opts.trace = Some(PathBuf::from(value("--trace")?)),
            "--accept-project-security" => opts.accept_project_security = true,
            "--trust-project" => opts.trust_project = true,
            "--remote-host" => remote_host = true,
            "--lan" => host.lan = true,
            "--port" => {
                let v = value("--port")?;
                host.port = Some(
                    v.parse()
                        .map_err(|e| CliError::Usage(format!("--port {v}: {e}")))?,
                );
            }
            "--config-dir" => config_dir = Some(PathBuf::from(value("--config-dir")?)),
            "--no-user-config" => no_user_config = true,
            other => return Err(CliError::Usage(format!("unknown flag `{other}`"))),
        }
    }
    if !remote_host && (host.lan || host.port.is_some()) {
        return Err(CliError::Usage(
            "--lan and --port configure the split-editor host: add --remote-host".into(),
        ));
    }
    let core = forge_editor::core::EditorCore::new();
    if opts.trust_project {
        // The launcher vouches for what the run opens (WP-34); the remote host too.
        forge_editor::core::EditorCore::set_trust_book(
            &core,
            std::sync::Arc::new(forge_editor::trust::TrustEvery),
        );
    }
    let failed = |e: forge_editor::EditorError| CliError::Step {
        step: "headless",
        detail: e.to_string(),
    };
    if remote_host {
        // stdin is the host console; a script (a file) prepares the project first.
        if script.as_deref() == Some("-") {
            return Err(CliError::Usage(
                "--remote-host reads the host console from stdin: give --script a file".into(),
            ));
        }
        {
            let mut out = std::io::stdout().lock();
            match script.as_deref() {
                Some(path) => {
                    let f = std::fs::File::open(path)
                        .map_err(|e| CliError::Usage(format!("--script {path}: {e}")))?;
                    run(
                        &core,
                        path,
                        Some(std::io::BufReader::new(f)),
                        &opts,
                        &mut out,
                    )
                    .map_err(failed)?;
                }
                None if opts.replay.is_some() => {
                    run(
                        &core,
                        "replay",
                        None::<std::io::StdinLock<'_>>,
                        &opts,
                        &mut out,
                    )
                    .map_err(failed)?;
                }
                None => {}
            }
        }
        host.config_dir = if no_user_config {
            None
        } else {
            config_dir.or_else(forge_editor::user_config::user_config_dir)
        };
        let mut out = std::io::stdout();
        return forge_remote::console::serve(
            &core,
            &host,
            std::io::BufReader::new(std::io::stdin()),
            &mut out,
        )
        .map_err(|e| CliError::Step {
            step: "remote-host",
            detail: e.to_string(),
        });
    }
    let mut out = std::io::stdout().lock();
    let r = match script.as_deref() {
        Some("-") => run(
            &core,
            "stdin",
            Some(std::io::stdin().lock()),
            &opts,
            &mut out,
        ),
        Some(path) => {
            let f = std::fs::File::open(path)
                .map_err(|e| CliError::Usage(format!("--script {path}: {e}")))?;
            run(
                &core,
                path,
                Some(std::io::BufReader::new(f)),
                &opts,
                &mut out,
            )
        }
        None if opts.replay.is_some() => run(
            &core,
            "replay",
            None::<std::io::StdinLock<'_>>,
            &opts,
            &mut out,
        ),
        None => run(
            &core,
            "stdin",
            Some(std::io::stdin().lock()),
            &opts,
            &mut out,
        ),
    };
    r.map(drop).map_err(failed)
}
