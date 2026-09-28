//! `cargo xtask` entry point. See `xtask::USAGE`.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match xtask::dispatch(&args) {
        Ok(out) => {
            let _ = std::io::stdout().write_all(out.as_bytes());
            ExitCode::SUCCESS
        }
        Err(e) => {
            let _ = std::io::stderr().write_all(e.to_string().as_bytes());
            ExitCode::FAILURE
        }
    }
}
