// timed-gates: exempt(timeouts waiting for a child process to answer or exit, hang guards; no time is asserted)
//! `test_headless_remote_host` (Ch.34 §34.4–§34.5, DoD M2-16): **the headless core serves the
//! split editor.** A workstation hosts a laptop with `forge --headless --remote-host` — no
//! window, no GPU, no panels — and a person at its terminal shows the pairing code and allows
//! the device.
//!
//! The test runs the real `forge` binary as that host (its own process, loopback, a config
//! folder of its own) and plays both people: at the host's console it types `pair`, reads the
//! code, and — once the device's request is announced — `allow`s it and grants the device
//! destructive edits (`grant dev-1 Command(Destructive)`); at the device it pairs with the
//! code over QUIC and opens a `RemoteBus` session. Then the split-editor parity script
//! (`crates/forge-remote/tests/common/parity_script.rs`, the one `test_split_editor_parity`
//! runs) goes over the wire, and the host's own project (its console's `state` digest: the
//! SHA-256 of the canonical wire form) must equal the same script run locally, byte for byte,
//! as must the device's replica; the undo histories and refusals must match. The host is then
//! restarted on the same config folder: it keeps its certificate and the pairing, so the
//! device reconnects without pairing again.
//!
//! `a_wrong_code_is_refused_at_the_headless_console`: a device that typed the wrong code is
//! announced, and allowing it is refused — the key exchange, not the console, decides.
//!
//! Positive control (W2): `positive_control_a_lossy_wire_fails_parity_against_the_headless_host`
//! sends floats as `f32` (`WireFaults::f32_floats`): the host's digest must then differ.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../crates/forge-remote/tests/common/parity_script.rs"]
mod parity_script;

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use forge_cmd::Issuer;
use forge_editor::client::BusClient;
use forge_editor::core::EditorCore;
use forge_remote::wire::project_digest;
use forge_remote::{ConnectOptions, Device, Identity, PairedHost, WireFaults};
use parity_script::{codes, drive, rows};

const WAIT: Duration = Duration::from_secs(30);

/// The `forge --headless --remote-host` process. Killed (by its own PID) if a test fails.
struct HostProc {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    log: Vec<String>,
    addr: SocketAddr,
    certificate: String,
}

impl HostProc {
    fn spawn(config_dir: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_forge"))
            .args(["--headless", "--remote-host", "--port", "0", "--config-dir"])
            .arg(config_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("forge starts");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines() {
                let Ok(l) = l else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        let mut h = Self {
            child,
            stdin,
            lines,
            log: Vec::new(),
            addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            certificate: String::new(),
        };
        let at = h.expect("the host's address", |l| {
            l.starts_with("forge: split-editor host on ")
        });
        let rest = at.trim_start_matches("forge: split-editor host on ");
        let (addr, tail) = rest.split_once(' ').expect("an address, then the exposure");
        h.addr = addr.parse().expect("a socket address");
        h.certificate = tail
            .rsplit_once("host certificate ")
            .expect("the host certificate")
            .1
            .to_string();
        h
    }

    fn send(&mut self, line: &str) {
        let s = self.stdin.as_mut().expect("the console is open");
        writeln!(s, "{line}").expect("the console reads");
        s.flush().expect("the console reads");
    }

    /// The next line matching `pred` (earlier ones are kept in the log).
    fn expect(&mut self, what: &str, pred: impl Fn(&str) -> bool) -> String {
        let end = Instant::now() + WAIT;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(l) => {
                    self.log.push(l.clone());
                    if pred(&l) {
                        return l;
                    }
                }
                Err(_) => panic!(
                    "the headless host never printed {what}; it printed:\n{}",
                    self.log.join("\n")
                ),
            }
        }
    }

    /// `quit` at the console; the process must exit cleanly.
    fn quit(mut self) -> ExitStatus {
        self.send("quit");
        self.expect("that it stopped", |l| {
            l == "forge: the split-editor host stopped"
        });
        let end = Instant::now() + WAIT;
        loop {
            if let Some(s) = self.child.try_wait().expect("the process is ours") {
                return s;
            }
            assert!(Instant::now() < end, "forge did not exit after `quit`");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for HostProc {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn config_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("forge-remote-host-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("a config folder");
    d
}

/// Pair a device named `name` with `host` the way two people do: `pair` at the host's
/// console, the code typed at the device (a wrong one if `wrong`), `allow` at the console.
fn pair(
    host: &mut HostProc,
    name: &str,
    wrong: bool,
) -> (
    Device,
    Result<PairedHost, forge_remote::RemoteError>,
    String,
) {
    host.send("pair");
    let shown = host.expect("a pairing code", |l| l.starts_with("forge: pairing code "));
    let code: String = shown
        .trim_start_matches("forge: pairing code ")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    assert_eq!(code.len(), 6, "{shown}");
    let device = Device::new(Identity::generate("forge-device").unwrap(), name);
    let d = device.clone();
    let addr = host.addr;
    let typed = match (wrong, code.as_str()) {
        (false, _) => code.clone(),
        (true, "000000") => "111111".into(),
        (true, _) => "000000".into(),
    };
    let waiting = std::thread::spawn(move || d.pair(addr, &typed, WAIT));
    let announced = format!("forge: pairing request from \u{201c}{name}\u{201d}");
    host.expect("the device's request", |l| l.starts_with(&announced));
    host.send(&format!("allow {name}"));
    let answer = host.expect("an answer to `allow`", |l| {
        l.starts_with("forge: paired ") || l.starts_with("forge: error: ")
    });
    let paired = waiting.join().expect("the device thread");
    (device, paired, answer)
}

/// What one run of the parity script produced.
struct Outcome {
    digest: String,
    next_key: u64,
    history: Vec<(String, String)>,
    refused: Vec<String>,
    replica: Option<String>,
}

fn local() -> Outcome {
    let core = EditorCore::new();
    let mut c = EditorCore::connect(&core, Issuer::Human { user: "ada".into() });
    let refused = drive(&mut c, &mut |c| codes(c.pump()));
    Outcome {
        digest: EditorCore::read(&core, project_digest).unwrap(),
        next_key: EditorCore::read(&core, |p| p.next_key().0),
        history: rows(c.history()),
        refused,
        replica: None,
    }
}

/// The host's `state` line: its digest and key allocator.
fn host_state(host: &mut HostProc) -> (String, u64) {
    host.send("state");
    let l = host.expect("the project state", |l| l.starts_with("forge: state "));
    let field = |k: &str| {
        l.split(' ')
            .find_map(|w| w.strip_prefix(k))
            .unwrap_or_else(|| panic!("no {k} in {l}"))
            .to_string()
    };
    (field("sha256="), field("next_key=").parse().unwrap())
}

/// The parity script from a device paired with the headless host process (its outcome, the
/// pinned host, and the device).
fn remote(host: &mut HostProc, faults: WireFaults) -> (Outcome, PairedHost, Device) {
    let (device, paired, answer) = pair(host, "studio-laptop", false);
    assert!(answer.contains("as dev-"), "{answer}");
    let paired = paired.unwrap_or_else(|e| panic!("{e}"));
    // Despawn and removals are destructive: the person at the host lets this device make them.
    host.send(&format!("grant {} Command(Destructive)", paired.device_id));
    let granted = host.expect("the grant", |l| l.contains(" may: "));
    assert!(granted.contains("Command(Destructive)"), "{granted}");
    let mut c = device
        .connect(
            &paired,
            ConnectOptions {
                faults,
                ..ConnectOptions::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    host.expect("the session announced", |l| l.contains(" started from "));
    let handle = c.handle();
    let refused = drive(&mut c, &mut |c| {
        assert!(
            handle.wait_settled(WAIT),
            "the host did not answer: {:?}",
            handle.closed()
        );
        codes(c.pump())
    });
    let (replica, _) = c.snapshot();
    let (digest, next_key) = host_state(host);
    let out = Outcome {
        digest,
        next_key,
        history: rows(c.history()),
        refused,
        replica: Some(project_digest(&replica).unwrap()),
    };
    drop(c);
    host.expect("the session's end", |l| l.ends_with(" ended"));
    (out, paired, device)
}

fn check(a: &Outcome, b: &Outcome) -> Result<(), String> {
    if a.digest != b.digest {
        return Err(format!(
            "PARITY BROKEN: the headless host's project ({}) is not the local one ({})",
            b.digest, a.digest
        ));
    }
    if a.next_key != b.next_key {
        return Err(format!(
            "the key allocators differ: {} vs {}",
            a.next_key, b.next_key
        ));
    }
    if a.history != b.history {
        return Err(format!(
            "the undo histories differ:\n{:?}\n{:?}",
            a.history, b.history
        ));
    }
    if a.refused != b.refused {
        return Err(format!(
            "the refusals differ: {:?} vs {:?}",
            a.refused, b.refused
        ));
    }
    if b.replica.as_ref() != Some(&b.digest) {
        return Err("the device's replica is not the headless host's project".into());
    }
    Ok(())
}

#[test]
fn a_device_pairs_with_the_headless_host_and_the_parity_script_matches() {
    let dir = config_dir("parity");
    let mut host = HostProc::spawn(&dir);
    assert!(
        host.addr.ip().is_loopback(),
        "loopback by default: {}",
        host.addr
    );
    let a = local();
    let (b, paired, device) = remote(&mut host, WireFaults::default());
    check(&a, &b).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        a.refused.len(),
        1,
        "exactly the ghost rename: {:?}",
        a.refused
    );
    assert!(host.quit().success());

    // A restarted host keeps its certificate and its paired devices: the device reconnects
    // without a code (a new core: an empty project).
    let mut again = HostProc::spawn(&dir);
    assert_eq!(
        again.certificate,
        forge_remote::identity::short(&paired.fingerprint),
        "the restarted host presents the certificate the device pinned"
    );
    let moved = PairedHost {
        addr: again.addr,
        ..paired
    };
    let c = device
        .connect(&moved, ConnectOptions::default())
        .unwrap_or_else(|e| panic!("the paired device reconnects: {e}"));
    again.expect("the session announced", |l| l.contains(" started from "));
    let (replica, _) = c.snapshot();
    let (digest, _) = host_state(&mut again);
    assert_eq!(project_digest(&replica).unwrap(), digest);
    drop(c);
    assert!(again.quit().success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_wrong_code_is_refused_at_the_headless_console() {
    let dir = config_dir("wrong-code");
    let mut host = HostProc::spawn(&dir);
    let (_, paired, answer) = pair(&mut host, "intruder", true);
    assert!(
        answer.starts_with("forge: error: ") && answer.contains("not the one on show"),
        "{answer}"
    );
    assert!(paired.is_err(), "a wrong code never pairs: {paired:?}");
    host.send("devices");
    host.expect("the device list", |l| l == "forge: no device is paired");
    assert!(host.quit().success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn positive_control_a_lossy_wire_fails_parity_against_the_headless_host() {
    let dir = config_dir("lossy");
    let mut host = HostProc::spawn(&dir);
    let a = local();
    let (b, _, _) = remote(&mut host, WireFaults { f32_floats: true });
    let e = check(&a, &b).expect_err("floats sent as f32 must break parity with the host");
    assert!(e.contains("PARITY BROKEN"), "{e}");
    assert!(host.quit().success());
    let _ = std::fs::remove_dir_all(&dir);
}
