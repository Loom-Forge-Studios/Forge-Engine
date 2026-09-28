//! Forge repo automation (Ch.30). Every subcommand is a pure check over files in the repo,
//! written as a library so each guard's positive control can be unit-tested (W2).

pub mod allocators;
pub mod dod;
pub mod fmt;
pub mod fp_rules;
pub mod gate;
pub mod gate_parity;
pub mod gpu_turn;
pub mod layering;
pub mod licence_audit;
pub mod plan_coverage;
pub mod premium;
pub mod timed_gates;
pub mod util;

pub const USAGE: &str = "\
cargo xtask <command>

commands:
  gate                     invariant rows: BOUND / AWAITING(reason) / UNBUILT(reason)
  dod [--milestone M0] [--ship M0]
                           every DoD id and its status; --ship fails on UNMET/unread
  plan-coverage            every directory is claimed by the master-plan repo tree
  gate-parity              `just verify` == CI's command list, same order (I12)
  licence-audit [--write]  permissive allow-lists, cargo-deny, NOTICES fresh (I13)
  fp-rules                 no target-cpu / target-feature / fast-math flags (Ch.3.3)
  layering                 crates/* depend on plugins/* only as listed hosts (WP-21, §32.8)
  allocators               defects.md and error-codes.md allocate each id once (W8)
  gpu-turn                 GPU-test crates have test-gpu-turn on forge-gpu (WP-19)
  timed-gates              wall-clock test files time alone in the serial group (WP-40)
  fmt [--check]            rustfmt every package, a few per call (`cargo fmt --all` overflows
                           Windows' command-line limit in deep checkouts, os error 206)
  premium-boundary [--write-debt]
                           no base crate depends on or names a premium one; the debt
                           in premium_debt.ron only shrinks (ADR 0059)
  premium-protections [BIN] [--drop-target-remap]
                           the release-premium profile is hardened and a built premium
                           binary carries no build-machine paths (+control) (ADR 0063);
                           --drop-target-remap is the break test (the gate must fail)
  export-public <out-dir> [--docs-only | --no-test]
                           write the public edition: premium cut out, leak-scanned; a code
                           export refuses while premium_debt.ron is not empty (ADR 0059)
";

/// Dispatch a command line (without the program name). Returns the text to print.
pub fn dispatch(args: &[String]) -> util::XResult<String> {
    let root = util::workspace_root();
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        Some("gate") => gate::run(&root),
        Some("dod") => dod::run(&root, &dod_options(rest)?),
        Some("plan-coverage") => plan_coverage::run(&root),
        Some("gate-parity") => gate_parity::run(&root),
        Some("licence-audit") => {
            let mode = if rest.iter().any(|a| a == "--write") {
                licence_audit::Mode::Write
            } else {
                licence_audit::Mode::Check
            };
            licence_audit::run(&root, mode)
        }
        Some("fp-rules") => fp_rules::run(&root),
        Some("layering") => layering::run(&root),
        Some("allocators") => allocators::run(&root),
        Some("gpu-turn") => gpu_turn::run(&root),
        Some("timed-gates") => timed_gates::run(&root),
        Some("fmt") => fmt::run(&root, rest.iter().any(|a| a == "--check")),
        Some("premium-boundary") => {
            premium::boundary::run(&root, rest.iter().any(|a| a == "--write-debt"))
        }
        Some("premium-protections") => {
            let (bin, opts) = premium::protections::parse_args(rest);
            premium::protections::run(&root, bin, opts)
        }
        Some("export-public") => premium::export::run(&root, rest),
        Some("help") | Some("--help") | Some("-h") | None => Ok(USAGE.to_string()),
        Some(other) => Err(util::Failure::one(format!(
            "unknown command `{other}`\n\n{USAGE}"
        ))),
    }
}

fn dod_options(args: &[String]) -> util::XResult<dod::Options> {
    let mut o = dod::Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let val = |it: &mut std::slice::Iter<'_, String>| {
            it.next()
                .cloned()
                .ok_or_else(|| util::Failure::one(format!("{a} needs a milestone, e.g. M0")))
        };
        match a.as_str() {
            "--milestone" => o.milestone = Some(val(&mut it)?),
            "--ship" => o.ship = Some(val(&mut it)?),
            other => {
                return Err(util::Failure::one(format!("dod: unknown flag `{other}`")));
            }
        }
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_fails_and_help_succeeds() {
        assert!(dispatch(&["nope".to_string()]).is_err());
        assert!(dispatch(&[]).unwrap().contains("gate-parity"));
    }

    #[test]
    fn dod_flags_parse() {
        let o = dod_options(&[
            "--milestone".into(),
            "M0".into(),
            "--ship".into(),
            "M1".into(),
        ])
        .unwrap();
        assert_eq!(o.milestone.as_deref(), Some("M0"));
        assert_eq!(o.ship.as_deref(), Some("M1"));
        assert!(dod_options(&["--ship".into()]).is_err());
    }
}
