//! `forge --headless` end to end (Ch.34.4): a script that builds a scene and plays it,
//! `--record` writing the session, a second process `--replay`ing the file alone (the scene
//! it embeds) to the same simulation hash, `--trace` writing a Perfetto trace that decodes,
//! and a tampered file failing with `SIM-0007`.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command;

fn forge(args: &[&str], stdin: Option<&str>) -> (bool, String) {
    use std::io::Write;
    let mut c = Command::new(env!("CARGO_BIN_EXE_forge"));
    c.args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = c.spawn().unwrap();
    {
        let mut i = child.stdin.take().unwrap();
        if let Some(s) = stdin {
            i.write_all(s.as_bytes()).unwrap();
        }
    }
    let o = child.wait_with_output().unwrap();
    (
        o.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    )
}

fn sim_hash(text: &str) -> String {
    text.lines()
        .find_map(|l| l.strip_prefix("sim hash "))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or_else(|| panic!("no sim hash in:\n{text}"))
        .to_owned()
}

const SCRIPT: &str = r#"# a ball thrown up, a spinning top
{"Spawn":{"name":"Ball","parent":null}}
{"SetProperty":{"entity":0,"path":"transform.position.local","value":{"Vec3":[0.0,1.5,0.0]}}}
{"SetProperty":{"entity":0,"path":"motion.velocity","value":{"Vec3":[2.0,9.0,0.0]}}}
{"SetProperty":{"entity":0,"path":"motion.acceleration","value":{"Vec3":[0.0,-9.81,0.0]}}}
{"Spawn":{"name":"Top","parent":null}}
{"SetProperty":{"entity":1,"path":"transform.position.local","value":{"Vec3":[3.0,0.0,0.0]}}}
{"SetProperty":{"entity":1,"path":"motion.spin_dps","value":{"Float":720.0}}}
play
run 45
input {"entity":0,"action":{"Impulse":[0.0,4.0,0.0]}}
run 30
pause
step 3
input {"entity":1,"action":{"SetSpin":-90.0}}
play
run 60
stop
"#;

#[test]
fn record_then_replay_the_file_alone_and_trace() {
    let d = std::env::temp_dir().join(format!("forge-headless-cli-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let rec = d.join("session.forgereplay");
    let trace = d.join("run.pftrace");
    let s = |p: &Path| p.to_str().unwrap().to_owned();

    let (ok, text) = forge(
        &[
            "--headless",
            "--script",
            "-",
            "--record",
            &s(&rec),
            "--trace",
            &s(&trace),
        ],
        Some(SCRIPT),
    );
    assert!(ok, "{text}");
    assert!(text.contains("Stopped after 138 step(s)"), "{text}");
    assert!(text.contains("budget sim: sim.step"), "{text}");
    let recorded = sim_hash(&text);

    let (ok, again) = forge(&["--headless", "--replay", &s(&rec)], None);
    assert!(ok, "{again}");
    assert!(again.contains("against the recorded scene"), "{again}");
    assert_eq!(sim_hash(&again), recorded);

    let bytes = std::fs::read(&trace).unwrap();
    let decoded = forge_trace::perfetto::read(&bytes).expect("the trace decodes");
    let steps = decoded
        .events
        .iter()
        .filter(|e| e.name.as_deref() == Some("sim.step"))
        .count();
    assert_eq!(steps, 138, "one sim.step slice per step");

    // A tampered checkpoint is a divergence, and the binary says where.
    let text = std::fs::read_to_string(&rec).unwrap();
    let line = text.lines().find(|l| l.starts_with("{\"check\"")).unwrap();
    let broken = line.replacen("\"hash\":\"", "\"hash\":\"0", 1);
    std::fs::write(&rec, text.replacen(line, &broken, 1)).unwrap();
    let (ok, out) = forge(&["--headless", "--replay", &s(&rec)], None);
    assert!(!ok, "{out}");
    assert!(out.contains("SIM-0007"), "{out}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_bad_play_line_is_refused_and_named() {
    let (ok, text) = forge(
        &["--headless"],
        Some(
            "run 5\nstep x\ninput {\"entity\":1}\nplay\ninput {\"entity\":4,\"action\":{\"SetSpin\":1.0}}\n",
        ),
    );
    assert!(ok, "refusals are reported, not fatal: {text}");
    assert!(
        text.contains("line 1: refused `run` needs a playing simulation"),
        "{text}"
    );
    assert!(
        text.contains("line 2: refused `step N` needs a step count"),
        "{text}"
    );
    assert!(
        text.contains("line 3: refused not a simulation input"),
        "{text}"
    );
    assert!(text.contains("line 5: refused SIM-0002"), "{text}");
    assert!(text.contains("4 refused"), "{text}");
}
