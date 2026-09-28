// timed-gates: exempt(timeouts waiting for a child process to answer or exit, hang guards; no time is asserted)
//! `test_editor_headless_host` (Ch.34 §34.4, M2-16): `forge-editor --headless --remote-host`
//! hosts the split editor with no window or GPU, like `forge --headless --remote-host` (whose
//! `test_headless_remote_host` runs the full parity script). The real binary starts on
//! loopback, a person at its console shows a code and allows a device, the device edits over
//! QUIC, and the host's project (its console's `state` digest) is the device's replica.
//!
//! Both binaries run the same `forge_remote::console::serve`; the W2 positive control (a lossy
//! wire must break parity with the host) lives in forge-cli's test. Here the digest is also
//! checked against the empty project's, so a session whose edit never reached the host's core
//! fails.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use forge_cmd::{EditorCommand, Project};
use forge_editor::client::BusClient;
use forge_remote::wire::project_digest;
use forge_remote::{ConnectOptions, Device, Identity};

const WAIT: Duration = Duration::from_secs(30);

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
fn the_editor_binary_hosts_headless_and_a_device_edits_through_it() {
    let mut child = Kill(
        Command::new(env!("CARGO_BIN_EXE_forge-editor"))
            .args([
                "--headless",
                "--remote-host",
                "--port",
                "0",
                "--no-user-config",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("forge-editor starts"),
    );
    let mut stdin = child.0.stdin.take().expect("piped stdin");
    let stdout = child.0.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    let mut log = Vec::new();
    let mut expect = |what: &str, pred: &dyn Fn(&str) -> bool| -> String {
        let end = Instant::now() + WAIT;
        loop {
            match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                Ok(l) => {
                    log.push(l.clone());
                    if pred(&l) {
                        return l;
                    }
                }
                Err(_) => panic!("forge-editor never printed {what}:\n{}", log.join("\n")),
            }
        }
    };
    let mut say = |l: &str| {
        writeln!(stdin, "{l}").expect("the console reads");
        stdin.flush().expect("the console reads");
    };

    let at = expect("its address", &|l| {
        l.starts_with("forge-editor: split-editor host on ")
    });
    let addr: SocketAddr = at
        .trim_start_matches("forge-editor: split-editor host on ")
        .split(' ')
        .next()
        .expect("an address")
        .parse()
        .expect("a socket address");
    assert!(addr.ip().is_loopback(), "loopback by default: {at}");

    say("pair");
    let shown = expect("a code", &|l| l.starts_with("forge-editor: pairing code "));
    let code: String = shown
        .trim_start_matches("forge-editor: pairing code ")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let device = Device::new(Identity::generate("forge-device").unwrap(), "laptop");
    let d = device.clone();
    let waiting = std::thread::spawn(move || d.pair(addr, &code, WAIT));
    expect("the request", &|l| {
        l.starts_with("forge-editor: pairing request from \u{201c}laptop\u{201d}")
    });
    say("allow laptop");
    expect("the pairing", &|l| l.starts_with("forge-editor: paired "));
    let paired = waiting
        .join()
        .expect("the device thread")
        .unwrap_or_else(|e| panic!("{e}"));

    let mut c = device
        .connect(&paired, ConnectOptions::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let handle = c.handle();
    c.apply(
        EditorCommand::Spawn {
            name: "Cube".into(),
            parent: None,
        },
        None,
    );
    assert!(handle.wait_settled(WAIT), "{:?}", handle.closed());
    let pumped = c.pump();
    assert!(pumped.refused.is_empty(), "{:?}", pumped.refused);
    let (replica, _) = c.snapshot();
    assert_eq!(replica.len(), 1, "the device sees its cube");

    say("state");
    let state = expect("the state", &|l| l.starts_with("forge-editor: state "));
    let digest = state
        .split(' ')
        .find_map(|w| w.strip_prefix("sha256="))
        .expect("a digest");
    assert_eq!(digest, project_digest(&replica).unwrap(), "{state}");
    assert_ne!(
        digest,
        project_digest(&Project::default()).unwrap(),
        "the edit reached the host's core"
    );
    drop(c);
    say("quit");
    expect("that it stopped", &|l| {
        l == "forge-editor: the split-editor host stopped"
    });
    let end = Instant::now() + WAIT;
    let status = loop {
        if let Some(s) = child.0.try_wait().expect("the process is ours") {
            break s;
        }
        assert!(
            Instant::now() < end,
            "forge-editor did not exit after `quit`"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
}
