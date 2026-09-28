//! The **host console**: the split-editor host with no GUI (Ch.34 §34.4, M2-16). `forge
//! --headless --remote-host` and `forge-editor --headless --remote-host` serve their core over
//! [`QuicTransport`] and take the Remote panel's host-side decisions from a person at a
//! terminal instead — so a workstation hosts a laptop without paying for a window, a GPU
//! device or the panels.
//!
//! ```text
//! pair                      show a one-time pairing code (type it on the other device)
//! requests                  the devices asking to pair
//! allow NAME | deny NAME    answer a device's request (a person here decides, as in the panel)
//! devices                   the paired devices, their certificates and capabilities
//! grant ID CAP | revoke ID CAP   what device ID's sessions may do (Command(Destructive), ...)
//! unpair ID                 forget a device (its sessions end)
//! sessions                  the connected devices' sessions
//! disconnect SESSION        end one session
//! exposure loopback|lan     where the host listens (typing `lan` here is the confirmation)
//! state                     the project's digest (SHA-256 of its canonical wire form)
//! held                      security settings an automation session's or a script's load held back
//! accept-held ID | discard-held ID   answer them (the human-only commands, audited)
//! trust                     what the open project carries that needs the person's trust
//! trust ID | distrust ID    answer that question (the Plugin manager's answer, audited)
//! status | help | quit
//! ```
//!
//! **The same decisions, the same writers.** Every command here is the call the Remote panel
//! makes: the transport's [`RemoteTransport::begin_pairing`] / [`RemoteTransport::answer`],
//! and the allow-listed [`RemotePairingStore`] (user config, audited under the person running
//! the console). Nothing here reaches project state: a remote session is a peer of the core
//! checked in the one grant table, exactly as with the GUI host.
//!
//! **Held security settings (WP-21).** A project an automation session (on a paired device, or one
//! a plugin hosts) or a script opens or pulls may carry plugin grants or a default automation
//! policy wider than what is in effect; the core holds them for a person (WP-33). With no GUI, this
//! console is where the person is: `held` lists them (a new proposal is announced as it appears),
//! and `accept-held` / `discard-held` send `forge.security.accept_held` / `discard_held` as the
//! person at the terminal — the same human-only, audited commands the Plugin manager and the
//! Plugin manager send, checked by the core against the proposal it holds.
//!
//! **Project trust (WP-36).** A project that carries plugin code, plugin grants or a wider
//! default automation policy runs and grants none of it until a person trusts it (ADR 0045
//! Amendments 1 and 2). Before WP-36 a headless host could only be trusted wholesale with
//! `--trust-project`; now the person at this console is asked like the Plugin manager's
//! person: a question is announced as it appears, `trust` shows what the project carries
//! (what changed since they trusted it first), and `trust ID` / `distrust ID` answer **that**
//! question — through `EditorCore::decide_trust_seen`, the Plugin manager's own call, so the
//! answer is audited as `trust` / `distrust` under the person's name, a trusted project's
//! held grants are accepted with the human-only `forge.security.accept_held`, and an answer
//! to a question that changed since it was shown (a pull in between) is refused. The answer
//! is kept in the core's trust book (this run only, on a host started without
//! `--trust-project`). No bus command reaches it: a paired device, an automation session or a
//! script cannot trust a project.
//!
//! **Idle costs nothing.** The console blocks on one channel fed by the terminal's reader
//! thread and the transport's change feed (a push, not a poll): with no input and no device
//! activity the process makes no wakeups. A pairing request or a session starting or ending
//! is printed as it happens. When the terminal's input ends (a service with no console) the
//! host keeps serving the devices already paired until the process is stopped.

use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use forge_editor::connect::remote::{Exposure, RemotePairingStore, RemoteTransport};
use forge_editor::core::{EditorCore, SharedCore};
use forge_plugin::Capability;
use forge_ui::UiWaker;

use crate::host::{HostConfig, QuicTransport};
use crate::{RemoteError, identity, wire};

/// How to host (see the module docs).
#[derive(Clone, Debug)]
pub struct ConsoleOptions {
    /// The UDP port; `None`: [`crate::DEFAULT_PORT`], or any free one if that is taken.
    pub port: Option<u16>,
    /// `--lan`: listen on the local network (recorded in the pairing config, as the Remote
    /// panel's confirmed choice). Without it the pairing config's exposure stands (loopback
    /// unless a person chose the LAN).
    pub lan: bool,
    /// The user config directory: the host identity and the pairing store live here, so a
    /// restarted host keeps its certificate and its paired devices. `None`: nothing kept.
    pub config_dir: Option<PathBuf>,
    /// Who answers at this console (the audit's "by").
    pub operator: String,
    /// What the console calls itself in its lines (`forge`, `forge-editor`).
    pub program: String,
    #[doc(hidden)]
    pub faults: ConsoleFaults,
}

forge_trace::control_switches! {
    /// W2 positive-control switches for the console's guards. Never set outside those tests.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct ConsoleFaults {
        /// `deny NAME` answers the request at the end of time, as before WP-20: the device is
        /// told the code expired and the code on show is withdrawn.
        pub deny_by_answer: bool,
        /// `grant` / `revoke` of what the device already holds / lacks goes to the store and
        /// reports the list as if it changed (before WP-20).
        pub grant_rewrites: bool,
        /// `accept-held` grants the held plugin grants one by one with `forge.plugin.grant`
        /// instead of answering the proposal (`test_console_held`'s control: the proposal stays
        /// held, the default automation policy is not restored, and no `accept_held` is audited).
        pub held_by_grant: bool,
        /// `trust ID` trusts every project by swapping the core's trust book (as
        /// `--trust-project` does) instead of answering the question through the audited call
        /// (the control of `the_console_answers_project_trust_through_the_audited_call`:
        /// nothing is audited and the held grants stay held).
        pub trust_by_book: bool,
    }
}

impl Default for ConsoleOptions {
    fn default() -> Self {
        Self {
            port: None,
            lan: false,
            config_dir: None,
            operator: operator_name(),
            program: "forge".into(),
            faults: ConsoleFaults::default(),
        }
    }
}

/// The person at this terminal, as the OS names them.
#[must_use]
pub fn operator_name() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "console".into())
}

enum Event {
    Line(String),
    Eof,
    Changed,
}

struct ChannelWaker(mpsc::Sender<Event>);

impl UiWaker for ChannelWaker {
    fn wake(&self) {
        let _ = self.0.send(Event::Changed);
    }
}

/// The console's help text.
pub const HELP: &str = "host console commands:
  pair                      show a one-time pairing code (type it on the other device)
  requests                  the devices asking to pair
  allow NAME | deny NAME    answer a device's pairing request
  devices                   the paired devices and what they may do
  grant ID CAP              let device ID's sessions do more (e.g. Command(Destructive))
  revoke ID CAP             take a capability back
  unpair ID                 forget a device (its sessions end)
  sessions                  the connected sessions
  disconnect SESSION        end one session
  exposure loopback|lan     where the host listens (lan: paired devices on the network)
  state                     the project's digest
  held                      security settings an automation session's or a script's load held back
  accept-held ID            accept them (as the person at this console; audited)
  discard-held ID           discard them
  trust                     what the open project carries that needs your trust
  trust ID                  trust it (its plugins run, its held grants are accepted; audited)
  distrust ID               do not trust it (its plugins stay out, its grants held; audited)
  status | help | quit";

/// Serve `core` to paired devices over QUIC, taking the host-side decisions from `input` (a
/// terminal) and writing what happens to `out`, until `quit` (see the module docs).
///
/// The terminal is read on a thread of its own (it blocks); `input` ending does not stop the
/// host.
pub fn serve(
    core: &SharedCore,
    opts: &ConsoleOptions,
    input: impl BufRead + Send + 'static,
    out: &mut dyn Write,
) -> Result<(), RemoteError> {
    let prog = opts.program.clone();
    let (mut store, err) =
        RemotePairingStore::open(opts.config_dir.as_deref(), EditorCore::audit_book(core));
    if let Some(e) = err {
        say(
            out,
            &format!("{prog}: {e} (no device is paired from it; the next change rewrites it)"),
        );
    }
    if opts.lan {
        store
            .set_exposure(Exposure::Lan, true, &opts.operator)
            .map_err(|e| RemoteError::Config(e.to_string()))?;
    }
    let exposure = store.exposure();
    let start = |port: u16| {
        QuicTransport::start(
            core.clone(),
            HostConfig {
                port,
                exposure,
                config_dir: opts.config_dir.clone(),
                ..HostConfig::default()
            },
        )
    };
    let mut transport = match opts.port {
        Some(p) => start(p)?,
        None => start(crate::DEFAULT_PORT).or_else(|e| {
            say(
                out,
                &format!(
                    "{prog}: port {} is taken ({e}); listening on another",
                    crate::DEFAULT_PORT
                ),
            );
            start(0)
        })?,
    };
    transport.attach(&mut store);
    let addr = transport
        .local_addr()
        .map_or_else(|| "?".into(), |a| a.to_string());
    say(
        out,
        &format!(
            "{prog}: split-editor host on {addr} ({}), host certificate {}",
            exposure.label(),
            identity::short(transport.fingerprint())
        ),
    );
    say(
        out,
        &format!("{prog}: type `pair` to show a pairing code, `help` for the console commands"),
    );

    let (tx, rx) = mpsc::channel::<Event>();
    let changes = transport.changes();
    changes.set_waker(Some(Arc::new(ChannelWaker(tx.clone()))));
    let reader = tx;
    std::thread::Builder::new()
        .name("forge-remote-console".into())
        .spawn(move || {
            for line in input.lines() {
                match line {
                    Ok(l) => {
                        if reader.send(Event::Line(l)).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = reader.send(Event::Eof);
        })
        .map_err(|e| RemoteError::Transport(format!("the console reader: {e}")))?;

    let mut seen = Seen::default();
    seen.update(core, &transport, out, &prog);
    let mut con = Console {
        core,
        transport: &mut transport,
        store: &mut store,
        opts,
    };
    // Blocks until a line or a change arrives; nothing wakes an idle host.
    while let Ok(ev) = rx.recv() {
        match ev {
            Event::Changed => {
                changes.consumed();
                seen.update(core, con.transport, out, &prog);
            }
            Event::Eof => say(
                out,
                &format!(
                    "{prog}: the console's input ended; serving the paired devices until the process is stopped"
                ),
            ),
            Event::Line(l) => {
                if con.line(l.trim(), out) == Flow::Quit {
                    break;
                }
                seen.update(core, con.transport, out, &prog);
            }
        }
    }
    changes.set_waker(None);
    say(out, &format!("{prog}: the split-editor host stopped"));
    Ok(())
}

fn say(out: &mut dyn Write, line: &str) {
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// What the console has already told the person (so each request or session is announced
/// once).
#[derive(Default)]
struct Seen {
    requests: BTreeSet<(String, String)>,
    sessions: BTreeSet<(String, String)>,
    /// The held security proposal last announced.
    held: Option<u64>,
    /// The project trust question last announced (WP-36).
    trust: Option<u64>,
}

impl Seen {
    fn update(&mut self, core: &SharedCore, t: &QuicTransport, out: &mut dyn Write, prog: &str) {
        let trust = EditorCore::project_trust_id(core);
        if trust != self.trust {
            if let Some(q) = EditorCore::project_trust(core).filter(|q| q.state.asks()) {
                say(
                    out,
                    &forge_ui::trf!(
                        "{prog}: {label} Type `trust` to see what it carries, then `trust {id}` or `distrust {id}` to answer.",
                        prog,
                        label = q.label(),
                        id = q.id
                    ),
                );
            }
            self.trust = trust;
        }
        let held = EditorCore::held_security_id(core);
        if held != self.held {
            if let Some(h) = EditorCore::held_security(core) {
                say(
                    out,
                    &format!(
                        "{prog}: {} — type `held` to see them, `accept-held {}` or `discard-held {}` to answer",
                        h.label(),
                        h.id,
                        h.id
                    ),
                );
            }
            self.held = held;
        }
        let requests: BTreeSet<(String, String)> = t
            .requests()
            .into_iter()
            .map(|r| (r.device, r.fingerprint))
            .collect();
        for (device, fp) in requests.difference(&self.requests) {
            say(
                out,
                &format!(
                    "{prog}: pairing request from \u{201c}{device}\u{201d} (device certificate {}): type `allow {device}` or `deny {device}`",
                    identity::short(fp)
                ),
            );
        }
        self.requests = requests;
        let sessions: BTreeSet<(String, String)> =
            t.sessions().into_iter().map(|s| (s.id, s.device)).collect();
        for (id, device) in sessions.difference(&self.sessions) {
            say(out, &format!("{prog}: session {id} started from {device}"));
        }
        for (id, device) in self.sessions.difference(&sessions) {
            say(out, &format!("{prog}: session {id} from {device} ended"));
        }
        self.sessions = sessions;
    }
}

#[derive(PartialEq, Eq)]
enum Flow {
    Go,
    Quit,
}

struct Console<'a> {
    core: &'a SharedCore,
    transport: &'a mut QuicTransport,
    store: &'a mut RemotePairingStore,
    opts: &'a ConsoleOptions,
}

/// A capability as the console accepts it: the display spelling (`Command(Destructive)`) or
/// its key (`command_destructive`).
fn capability(s: &str) -> Result<Capability, String> {
    Capability::from_key(s).map_or_else(|| s.parse::<Capability>().map_err(|e| e.to_string()), Ok)
}

impl Console<'_> {
    fn now(&self) -> u64 {
        EditorCore::audit_book(self.core).now_ms()
    }

    fn line(&mut self, l: &str, out: &mut dyn Write) -> Flow {
        let prog = self.opts.program.clone();
        let who = self.opts.operator.clone();
        let (word, rest) = l.split_once(' ').unwrap_or((l, ""));
        let rest = rest.trim();
        let r: Result<String, String> = match word {
            "" => return Flow::Go,
            "quit" | "exit" => return Flow::Quit,
            "help" | "?" => Ok(HELP.into()),
            "status" => Ok(self.status()),
            "pair" => {
                let o = self.transport.begin_pairing(self.now());
                if o.code.is_empty() {
                    Err("no pairing code could be made (the OS's random source failed)".into())
                } else {
                    Ok(format!(
                        "pairing code {} \u{2014} type it on the other device within {} s \
                         (forge-editor --connect {} --pair {})",
                        o.code,
                        forge_editor::connect::remote::PAIRING_CODE_MS / 1000,
                        self.transport.address(self.store.exposure()),
                        o.code
                    ))
                }
            }
            "requests" => {
                let r = self.transport.requests();
                Ok(if r.is_empty() {
                    "no device is asking to pair".into()
                } else {
                    r.iter()
                        .map(|q| {
                            format!(
                                "request: \u{201c}{}\u{201d} (device certificate {})",
                                q.device,
                                identity::short(&q.fingerprint)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            "allow" => self.allow(rest, &who),
            "deny" => self.deny(rest, &who),
            "devices" => Ok(self.devices()),
            "grant" | "revoke" => self.capabilities(word == "grant", rest, &who),
            "unpair" => self
                .store
                .unpair(rest, &who)
                .map(|()| format!("unpaired {rest}"))
                .map_err(|e| e.to_string()),
            "sessions" => {
                let s = self.transport.sessions();
                Ok(if s.is_empty() {
                    "no session is connected".into()
                } else {
                    s.iter()
                        .map(|s| {
                            format!(
                                "session {} from {}: {} commands",
                                s.id, s.device, s.commands
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            "disconnect" => {
                if self.transport.disconnect(rest) {
                    Ok(format!("disconnected {rest}"))
                } else {
                    Err(format!("no session {rest}"))
                }
            }
            "exposure" => {
                let e = match rest {
                    "loopback" => Ok(Exposure::Loopback),
                    "lan" => Ok(Exposure::Lan),
                    other => Err(format!("`exposure {other}`: say `loopback` or `lan`")),
                };
                e.and_then(|e| {
                    // Typing `lan` at this console is the person's confirmation.
                    self.store
                        .set_exposure(e, true, &who)
                        .map_err(|x| x.to_string())?;
                    Ok(self.status())
                })
            }
            "held" => Ok(held_text(self.core)),
            "accept-held" | "discard-held" => self.answer_held(word == "accept-held", rest, &who),
            "trust" if rest.is_empty() => Ok(trust_text(self.core)),
            "trust" | "distrust" => self.decide_trust(word == "trust", rest, &who),
            "state" => EditorCore::read(self.core, |p| {
                wire::project_digest(p).map(|d| {
                    format!(
                        "state sha256={d} next_key={} entities={}",
                        p.next_key().0,
                        p.len()
                    )
                })
            })
            .map_err(|e| e.to_string()),
            other => Err(format!("unknown command `{other}` (try `help`)")),
        };
        match r {
            Ok(t) => {
                for line in t.lines() {
                    say(out, &format!("{prog}: {line}"));
                }
            }
            Err(e) => say(out, &format!("{prog}: error: {e}")),
        }
        Flow::Go
    }

    /// `accept-held ID` / `discard-held ID`: the person at the console answers the held
    /// proposal with the human-only command (see the module docs).
    fn answer_held(&mut self, accept: bool, rest: &str, who: &str) -> Result<String, String> {
        let id: u64 = rest.trim().trim_start_matches('#').parse().map_err(|_| {
            format!(
                "say which proposal: `{}-held ID` (`held` lists it)",
                if accept { "accept" } else { "discard" }
            )
        })?;
        if accept && self.opts.faults.held_by_grant() {
            // The W2 control's fault: grants, not an answer.
            let h = EditorCore::held_security(self.core).ok_or("no security settings are held")?;
            let mut person = EditorCore::connect(
                self.core,
                forge_cmd::Issuer::Human {
                    user: who.to_string(),
                },
            );
            for i in &h.items {
                if let Some((p, c)) = forge_editor::security::parse_grant_key(&i.key) {
                    forge_editor::client::BusClient::apply(
                        &mut person,
                        forge_editor::security::grant_command(&p, c),
                        None,
                    );
                }
            }
            let _ = forge_editor::client::BusClient::pump(&mut person);
            return Ok(format!(
                "accepted {} held security setting(s)",
                h.items.len()
            ));
        }
        EditorCore::answer_held(self.core, who, id, accept)
    }

    /// `trust ID` / `distrust ID`: the person at the console answers the project's trust
    /// question they were shown, through the Plugin manager's audited call (see the module
    /// docs).
    fn decide_trust(&mut self, trust: bool, rest: &str, who: &str) -> Result<String, String> {
        let id: u64 = rest.trim().trim_start_matches('#').parse().map_err(|_| {
            forge_ui::trf!(
                "say which question: `{word} ID` (`trust` shows it)",
                word = if trust { "trust" } else { "distrust" }
            )
        })?;
        let answer = if trust {
            forge_editor::trust::Trust::Trusted
        } else {
            forge_editor::trust::Trust::Untrusted
        };
        if trust
            && self.opts.faults.trust_by_book()
            && EditorCore::project_trust_id(self.core) == Some(id)
        {
            // The W2 control's fault: every project trusted, no answer given.
            EditorCore::set_trust_book(self.core, Arc::new(forge_editor::trust::TrustEvery));
            return Ok(forge_ui::trf!("trusted question #{id}", id));
        }
        EditorCore::decide_trust_seen(self.core, who, answer, id)
    }

    fn status(&self) -> String {
        let e = self.store.exposure();
        format!(
            "host on {} ({}), {} paired, {} connected; {}",
            self.transport
                .local_addr()
                .map_or_else(|| "?".into(), |a| a.to_string()),
            e.label(),
            self.store.devices().len(),
            self.transport.sessions().len(),
            self.transport.backend()
        )
    }

    /// Allow `device`'s request: the Remote panel's Allow, at a terminal.
    fn allow(&mut self, device: &str, who: &str) -> Result<String, String> {
        let now = self.now();
        let answer = self.transport.answer(device, now);
        // The device's certificate, as the transport verified it.
        let verified = answer
            .as_ref()
            .ok()
            .and_then(|d| self.transport.take_verified(d));
        match answer {
            Ok(name) => self
                .store
                .pair_verified(&name, verified.as_deref(), who, now)
                .map(|d| {
                    format!(
                        "paired \u{201c}{}\u{201d} as {} (may: {})",
                        d.name,
                        d.id,
                        caps(&d.capabilities)
                    )
                })
                .map_err(|e| e.to_string()),
            Err(e) => {
                self.store.denied(device, who, &e.to_string());
                Err(e.to_string())
            }
        }
    }

    /// Deny `device`'s request: the Remote panel's Deny, at a terminal. The device is told
    /// that the person here denied it (not a code problem), the refusal is audited, and the
    /// pairing code stays on show for another device (the person may `pair` again for a
    /// fresh one).
    fn deny(&mut self, device: &str, who: &str) -> Result<String, String> {
        if !self.transport.requests().iter().any(|q| q.device == device) {
            return Err(format!("no pairing request from \u{201c}{device}\u{201d}"));
        }
        let why = format!("{who} denied the pairing request at the host console");
        if self.opts.faults.deny_by_answer() {
            // The fault (before WP-20): an "answer" at the end of time — the device hears
            // that the code expired, and the code on show is withdrawn.
            let _ = self.transport.answer(device, u64::MAX);
        } else {
            self.transport
                .deny(device, &why)
                .map_err(|e| e.to_string())?;
        }
        self.store.denied(device, who, "denied at the host console");
        Ok(format!(
            "denied \u{201c}{device}\u{201d} (the pairing code stays on show)"
        ))
    }

    fn devices(&self) -> String {
        let d = self.store.devices();
        if d.is_empty() {
            return "no device is paired".into();
        }
        d.iter()
            .map(|d| {
                format!(
                    "{} \u{201c}{}\u{201d} certificate {} allowed by {} (may: {})",
                    d.id,
                    d.name,
                    identity::short(&d.fingerprint),
                    d.allowed_by,
                    caps(&d.capabilities)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn capabilities(&mut self, add: bool, rest: &str, who: &str) -> Result<String, String> {
        let (id, cap) = rest
            .split_once(' ')
            .ok_or_else(|| "say `grant ID CAP` or `revoke ID CAP`".to_string())?;
        let cap = capability(cap.trim())?;
        let mut now = self
            .store
            .devices()
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.capabilities.clone())
            .ok_or_else(|| format!("no paired device {id}"))?;
        // Granting what it holds, or revoking what it does not: nothing to change, and
        // nothing is written or audited (WP-20).
        if now.contains(&cap) == add && !self.opts.faults.grant_rewrites() {
            return Ok(format!(
                "{id} {} {cap}; nothing changed (may: {})",
                if add { "already may" } else { "may not" },
                caps(&now)
            ));
        }
        if add {
            now.push(cap);
        } else {
            now.retain(|c| *c != cap);
        }
        self.store
            .set_capabilities(id, &now, who)
            .map_err(|e| e.to_string())?;
        now.sort();
        now.dedup();
        Ok(format!("{id} may: {}", caps(&now)))
    }
}

/// What the core holds for a person, one line each (the `held` command).
fn held_text(core: &SharedCore) -> String {
    match EditorCore::held_security(core) {
        None => "no security settings are held (a person's own opens take effect; an automation session's or a script's wait here)".into(),
        Some(h) => {
            let mut lines = vec![format!("held #{}: {}", h.id, h.label())];
            lines.extend(h.items.iter().map(|i| format!("  {}", i.line())));
            lines.push(format!(
                "type `accept-held {}` to let them take effect, or `discard-held {}` to keep what is in effect",
                h.id, h.id
            ));
            lines.join("
")
        }
    }
}

/// The open project's trust question, one line each (the `trust` command, WP-36).
fn trust_text(core: &SharedCore) -> String {
    let Some(q) = EditorCore::project_trust(core) else {
        return forge_ui::tr!(
            "the open project carries no plugins, plugin grants or wider automation capabilities: nothing to trust"
        )
        .into();
    };
    let mut lines = vec![forge_ui::trf!(
        "trust #{id}: {label}",
        id = q.id,
        label = q.label()
    )];
    lines.extend(q.lines().into_iter().map(|l| format!("  {l}")));
    lines.push(forge_ui::trf!(
        "remembered: {book}",
        book = EditorCore::trust_book_describe(core)
    ));
    lines.push(forge_ui::trf!(
        "type `trust {id}` to trust it, or `distrust {id}` not to",
        id = q.id
    ));
    lines.join("\n")
}

fn caps(c: &[Capability]) -> String {
    if c.is_empty() {
        return "nothing".into();
    }
    c.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_parse_in_both_spellings() {
        assert_eq!(
            capability("Command(Destructive)"),
            Ok(Capability::Command(forge_plugin::CommandClass::Destructive))
        );
        assert_eq!(
            capability("command_destructive"),
            Ok(Capability::Command(forge_plugin::CommandClass::Destructive))
        );
        assert!(capability("root").is_err());
    }

    // ---- WP-20: `deny` tells the device the real reason; a no-op grant writes nothing ----
    //
    // Driven through the console's own command handling over the real QUIC transport, with
    // real devices pairing from other threads. Positive controls (W2): the pre-WP-20 deny
    // (`ConsoleFaults::deny_by_answer`: the device hears "the pairing code expired" and the
    // code is withdrawn) and grant (`ConsoleFaults::grant_rewrites`: a grant of a held
    // capability reports a change) each fail their check.

    use std::time::Duration;

    use crate::device::wait_for;
    use crate::{Device, HostConfig, Identity};
    use forge_editor::connect::audit::AuditQuery;

    struct Rig {
        core: SharedCore,
        transport: QuicTransport,
        store: RemotePairingStore,
        opts: ConsoleOptions,
    }

    impl Rig {
        fn new(faults: ConsoleFaults) -> Self {
            let core = EditorCore::new();
            let transport = QuicTransport::start(
                core.clone(),
                HostConfig {
                    port: 0,
                    ..HostConfig::default()
                },
            )
            .unwrap_or_else(|e| panic!("{e}"));
            let (mut store, err) = RemotePairingStore::open(None, EditorCore::audit_book(&core));
            assert!(err.is_none(), "{err:?}");
            transport.attach(&mut store);
            Self {
                core,
                transport,
                store,
                opts: ConsoleOptions {
                    operator: "ada".into(),
                    faults,
                    ..ConsoleOptions::default()
                },
            }
        }

        /// Type `line` at the console; what it printed.
        fn line(&mut self, line: &str) -> String {
            let mut out = Vec::new();
            let mut con = Console {
                core: &self.core,
                transport: &mut self.transport,
                store: &mut self.store,
                opts: &self.opts,
            };
            con.line(line, &mut out);
            String::from_utf8_lossy(&out).into_owned()
        }

        /// A device named `name` types `code` on another thread; waits for its request.
        fn request(
            &self,
            name: &str,
            code: &str,
        ) -> std::thread::JoinHandle<Result<crate::PairedHost, RemoteError>> {
            let device = Device::new(
                Identity::generate("forge-device").unwrap_or_else(|e| panic!("{e}")),
                name,
            );
            let addr = self
                .transport
                .local_addr()
                .unwrap_or_else(|| panic!("the host listens"));
            let code = code.to_owned();
            let t = std::thread::spawn(move || device.pair(addr, &code, Duration::from_secs(30)));
            let n = name.to_owned();
            assert!(
                wait_for(Duration::from_secs(10), || self
                    .transport
                    .requests()
                    .iter()
                    .any(|r| r.device == n)),
                "{name}'s request never arrived"
            );
            t
        }

        fn audit_lines(&self) -> u64 {
            EditorCore::audit_book(&self.core).len()
        }
    }

    fn check_deny(faults: ConsoleFaults) -> Result<(), String> {
        let mut rig = Rig::new(faults);
        rig.line("pair");
        let code = rig
            .transport
            .offer()
            .map(|o| o.code.clone())
            .ok_or("no code on show")?;
        let laptop = rig.request("laptop", &code);
        let said = rig.line("deny laptop");
        if !said.contains("denied") {
            return Err(format!("the console did not deny: {said}"));
        }
        let why = match laptop.join().map_err(|_| "the device thread panicked")? {
            Ok(_) => return Err("a denied device was paired".into()),
            Err(e) => e.to_string(),
        };
        if !why.contains("ada denied the pairing request") || why.contains("expired") {
            return Err(format!("the device was not told the real reason: {why}"));
        }
        let denied = EditorCore::audit_book(&rig.core)
            .query(&AuditQuery::parse("pairing_denied"))
            .len();
        if denied == 0 {
            return Err("the denial was not audited".into());
        }
        // The code stays on show: another device typing it is paired when allowed.
        if rig.transport.offer().map(|o| o.code.as_str()) != Some(code.as_str()) {
            return Err("the pairing code on show was withdrawn by the denial".into());
        }
        let tablet = rig.request("tablet", &code);
        let said = rig.line("allow tablet");
        tablet
            .join()
            .map_err(|_| "the device thread panicked")?
            .map_err(|e| format!("the code on show did not pair the next device: {e} ({said})"))?;
        Ok(())
    }

    #[test]
    fn deny_tells_the_device_why_and_keeps_the_code_on_show() {
        check_deny(ConsoleFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn positive_control_deny_by_answer_says_the_code_expired() {
        let e = check_deny(ConsoleFaults {
            deny_by_answer: true,
            ..ConsoleFaults::default()
        })
        .expect_err("the answer-based deny must fail the check");
        assert!(e.contains("real reason") || e.contains("withdrawn"), "{e}");
    }

    fn check_grant(faults: ConsoleFaults) -> Result<(), String> {
        let mut rig = Rig::new(faults);
        rig.line("pair");
        let code = rig
            .transport
            .offer()
            .map(|o| o.code.clone())
            .ok_or("no code on show")?;
        let laptop = rig.request("laptop", &code);
        rig.line("allow laptop");
        laptop
            .join()
            .map_err(|_| "the device thread panicked")?
            .map_err(|e| e.to_string())?;
        let id = rig
            .store
            .devices()
            .first()
            .map(|d| d.id.clone())
            .ok_or("nothing paired")?;
        // A real change is written and audited...
        let before = rig.audit_lines();
        let said = rig.line(&format!("grant {id} Command(Destructive)"));
        if rig.audit_lines() != before + 1 || !said.contains("Command(Destructive)") {
            return Err(format!("a real grant was not written once: {said}"));
        }
        // ...granting it again changes nothing, and says so.
        let before = rig.audit_lines();
        let said = rig.line(&format!("grant {id} command_destructive"));
        if rig.audit_lines() != before {
            return Err(format!("a grant of a held capability was audited: {said}"));
        }
        if !said.contains("already may Command(Destructive); nothing changed") {
            return Err(format!("a no-op grant did not say so: {said}"));
        }
        let said = rig.line(&format!("revoke {id} Net(Outbound)"));
        if !said.contains("may not Net(Outbound); nothing changed") {
            return Err(format!("a no-op revoke did not say so: {said}"));
        }
        Ok(())
    }

    #[test]
    fn a_grant_of_a_held_capability_writes_nothing_and_says_so() {
        check_grant(ConsoleFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn positive_control_a_rewriting_grant_fails() {
        let e = check_grant(ConsoleFaults {
            grant_rewrites: true,
            ..ConsoleFaults::default()
        })
        .expect_err("a grant that reports a change must fail the check");
        assert!(e.contains("did not say so"), "{e}");
    }

    // ---- WP-21: the console shows held security settings and answers them ----------------
    //
    // A script opens a project whose settings file carries a plugin grant: the core holds
    // it. At the console, `held` lists it and `accept-held ID` answers it with the human-only
    // `forge.security.accept_held` as the person at the terminal: the grant takes effect,
    // the proposal is gone, the answer is audited by that person. `discard-held` of a stale id
    // is refused with the reason. Control: a console that grants the held plugin grants
    // instead of answering the proposal (`ConsoleFaults::held_by_grant`) leaves it held.

    fn check_held(faults: ConsoleFaults) -> Result<(), String> {
        use forge_editor::client::BusClient;
        use forge_editor::project::{ProjectOp, Template};
        use forge_editor::security;
        use forge_plugin::{FsScope, PluginId, Principal};
        use forge_project::format::{MANIFEST_PATH, ProjectDoc, SCENE_PATH, SETTINGS_PATH};

        let mut rig = Rig::new(faults);
        let dir = std::env::temp_dir().join(format!(
            "forge-wp21-console-held-{}-{}",
            std::process::id(),
            faults.held_by_grant()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let location = format!("file:{}", dir.display());
        let mut person =
            EditorCore::connect(&rig.core, forge_cmd::Issuer::Human { user: "ada".into() });
        person.apply(
            ProjectOp::Create {
                location: location.clone(),
                name: "Held".into(),
                template: Template::ThreeD,
                discard_unsaved: true,
            }
            .command(),
            None,
        );
        person.apply(forge_editor::project::save_command("start"), None);
        if let Some(r) = person.pump().refused.first() {
            return Err(format!("setup: {}", r.rejection));
        }
        EditorCore::wait_transfers(&rig.core);
        let rivers =
            Principal::Plugin(PluginId::new("com.example.rivers").map_err(|e| e.to_string())?);
        let read = Capability::Fs(FsScope::ProjectRead);
        let text = |p: &str| std::fs::read_to_string(dir.join(p)).map_err(|e| format!("{p}: {e}"));
        let (_, mut doc) = ProjectDoc::decode(
            &text(MANIFEST_PATH)?,
            Some(&text(SETTINGS_PATH)?),
            Some(&text(SCENE_PATH)?),
        )
        .map_err(|e| e.to_string())?;
        doc.settings.insert(
            security::grant_key(&rivers, read).ok_or("no grant key")?,
            forge_cmd::Value::Bool(true),
        );
        for (path, body) in doc.encode().map_err(|e| e.to_string())? {
            if path == SETTINGS_PATH {
                std::fs::write(dir.join(path), body).map_err(|e| e.to_string())?;
            }
        }
        // A script opens it: the grant is held.
        let mut script = EditorCore::connect(
            &rig.core,
            forge_cmd::Issuer::Script {
                path: "ci.forge".into(),
            },
        );
        script.apply(
            ProjectOp::Open {
                location,
                discard_unsaved: true,
            }
            .command(),
            None,
        );
        if let Some(r) = script.pump().refused.first() {
            return Err(format!("the script's open: {}", r.rejection));
        }
        EditorCore::wait_transfers(&rig.core);
        let h = EditorCore::held_security(&rig.core).ok_or("the script's open held nothing")?;
        if EditorCore::grants(&rig.core).has(&rivers, read) {
            return Err("the script's open granted the plugin".into());
        }
        let said = rig.line("held");
        if !said.contains(&format!("held #{}", h.id)) || !said.contains("com.example.rivers") {
            return Err(format!("`held` does not list the proposal: {said}"));
        }
        let said = rig.line(&format!("discard-held {}", h.id + 7));
        if !said.contains("error") || !said.contains("is not held") {
            return Err(format!(
                "a stale id was not refused with the reason: {said}"
            ));
        }
        let said = rig.line(&format!("accept-held {}", h.id));
        if !EditorCore::grants(&rig.core).has(&rivers, read) {
            return Err(format!("the console's accept did not grant: {said}"));
        }
        if EditorCore::held_security(&rig.core).is_some() {
            return Err(format!(
                "the proposal outlived the console's accept: {said}"
            ));
        }
        let by_ada = EditorCore::audit_book(&rig.core)
            .query(&AuditQuery::parse("accept_held"))
            .into_iter()
            .any(|r| r.event == "accept_held" && r.user == "ada");
        if !by_ada {
            return Err("the console's accept is not audited as the person's".into());
        }
        let said = rig.line("held");
        if !said.contains("no security settings are held") {
            return Err(format!("`held` after the answer: {said}"));
        }
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn the_console_lists_and_answers_held_security_settings() {
        check_held(ConsoleFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn positive_control_a_console_that_grants_instead_of_answering_fails() {
        let e = check_held(ConsoleFaults {
            held_by_grant: true,
            ..ConsoleFaults::default()
        })
        .expect_err("granting instead of answering must fail the check");
        assert!(e.contains("outlived"), "{e}");
    }

    // ---- WP-36: `trust` / `distrust` at the console, through the audited human-only path ---
    //
    // A teammate's project in a folder grants plugin `com.example.rivers` (a clone of it).
    // The person at the console opens it: the grant is held for trust and the question is
    // announced. `trust` shows it; `trust` of a stale id is refused with the reason; `trust ID`
    // answers it through `EditorCore::decide_trust_seen` — the grant takes effect (the held
    // proposal accepted with the human-only command), the answer is audited as `trust` by the
    // person; `distrust ID` is audited as `distrust`. Control: a console that trusts by
    // swapping the trust book (`ConsoleFaults::trust_by_book`, what `--trust-project` does)
    // audits nothing and leaves the grant held.

    fn check_trust(faults: ConsoleFaults) -> Result<(), String> {
        use forge_editor::client::BusClient;
        use forge_editor::project::{ProjectOp, Template};
        use forge_editor::security;
        use forge_plugin::{FsScope, PluginId, Principal};

        let mut rig = Rig::new(faults);
        let dir = std::env::temp_dir().join(format!(
            "forge-wp36-console-trust-{}-{}",
            std::process::id(),
            faults.trust_by_book()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let location = format!("file:{}", dir.display());
        let rivers =
            Principal::Plugin(PluginId::new("com.example.rivers").map_err(|e| e.to_string())?);
        let read = Capability::Fs(FsScope::ProjectRead);
        // A teammate's editor made the project and its grant.
        let teammate = EditorCore::new();
        let mut mate = EditorCore::connect(
            &teammate,
            forge_cmd::Issuer::Human {
                user: "mate".into(),
            },
        );
        mate.apply(
            ProjectOp::Create {
                location: location.clone(),
                name: "Cloned".into(),
                template: Template::ThreeD,
                discard_unsaved: true,
            }
            .command(),
            None,
        );
        mate.apply(security::grant_command(&rivers, read), None);
        mate.apply(forge_editor::project::save_command("grant"), None);
        if let Some(r) = mate.pump().refused.first() {
            return Err(format!("setup: {}", r.rejection));
        }
        EditorCore::wait_transfers(&teammate);
        // The person at the console opens it.
        let mut person =
            EditorCore::connect(&rig.core, forge_cmd::Issuer::Human { user: "ada".into() });
        person.apply(
            ProjectOp::Open {
                location,
                discard_unsaved: true,
            }
            .command(),
            None,
        );
        if let Some(r) = person.pump().refused.first() {
            return Err(format!("the person's open: {}", r.rejection));
        }
        EditorCore::wait_transfers(&rig.core);
        if EditorCore::grants(&rig.core).has(&rivers, read) {
            return Err("setup: the untrusted project's grant took effect".into());
        }
        let q = EditorCore::project_trust(&rig.core).ok_or("setup: no trust question")?;
        // Announced as it appears.
        let mut seen = Seen::default();
        let mut out = Vec::new();
        seen.update(&rig.core, &rig.transport, &mut out, "forge");
        let announced = String::from_utf8_lossy(&out).into_owned();
        if !announced.contains(&format!("`trust {}`", q.id)) {
            return Err(format!("the question was not announced: {announced}"));
        }
        let said = rig.line("trust");
        if !said.contains(&format!("trust #{}", q.id)) || !said.contains("com.example.rivers") {
            return Err(format!("`trust` does not show the question: {said}"));
        }
        let said = rig.line(&format!("trust {}", q.id + 7));
        if !said.contains("error") || !said.contains("changed since it was shown") {
            return Err(format!("a stale question id was not refused: {said}"));
        }
        let book = EditorCore::audit_book(&rig.core);
        let audited = |event: &str| {
            book.query(&AuditQuery::parse(event))
                .into_iter()
                .any(|r| r.event == event && r.user == "ada")
        };
        let said = rig.line(&format!("trust {}", q.id));
        let mut broken = Vec::new();
        if !audited("trust") {
            broken.push(format!(
                "the console's trust is not audited as the person's: {said}"
            ));
        }
        if !EditorCore::grants(&rig.core).has(&rivers, read)
            || EditorCore::held_security(&rig.core).is_some()
        {
            broken.push(format!("trusting did not accept the held grant: {said}"));
        }
        if !broken.is_empty() {
            return Err(broken.join("; "));
        }
        let q = EditorCore::project_trust(&rig.core).ok_or("no trust question")?;
        let said = rig.line(&format!("distrust {}", q.id));
        if !audited("distrust") {
            return Err(format!("the console's distrust is not audited: {said}"));
        }
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn the_console_answers_project_trust_through_the_audited_call() {
        check_trust(ConsoleFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn positive_control_a_console_that_trusts_by_the_book_fails() {
        let e = check_trust(ConsoleFaults {
            trust_by_book: true,
            ..ConsoleFaults::default()
        })
        .expect_err("trusting by the book must fail the check");
        assert!(
            e.contains("not audited") && e.contains("did not accept the held grant"),
            "{e}"
        );
    }
}
