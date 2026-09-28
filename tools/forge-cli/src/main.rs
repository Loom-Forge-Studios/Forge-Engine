//! `forge` — the headless engine binary (Ch.30, Ch.34.4): [`forge_cli::app`]'s commands.

fn main() -> std::process::ExitCode {
    forge_cli::app::main_with(&[])
}
