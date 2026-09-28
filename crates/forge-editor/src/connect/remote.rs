//! **Remote connect** (Ch.21 §21.21, Ch.34; DoD M2-55): pair a device, list remote sessions,
//! show the link's latency — over the [`RemoteTransport`] trait.
//!
//! * **Pairing needs a human on both devices.** This editor shows a one-time code
//!   ([`RemoteTransport::begin_pairing`]); the person at the other device types it there; the
//!   device's request arrives here ([`PairingRequest`]) and a person here allows or denies
//!   it. A wrong or expired code is refused (`EDITOR-0012`). No automation or script path reaches
//!   any of it.
//! * **Pairing is user config**, written only by the allow-listed [`RemotePairingStore`]
//!   (`<config>/remote_pairing.ron`, through the editor's atomic user-config writer), and
//!   audited: every pairing, unpairing and exposure change goes to the [`AuditBook`]. A
//!   paired device is a per-machine credential (Ch.37 §37.6), so it is never project state.
//! * **Loopback by default.** LAN exposure is an explicit choice, refused without its
//!   confirmation (Ch.34 §34.5). Internet exposure needs a written acknowledgement in the
//!   config and is not offered here.
//! * **The latency indicator is a live feed** ([`RemoteTransport::latency`]) the panel
//!   refreshes at most twice a second, only while visible (§21.11).
//!
//! The split editor's QUIC transport is `forge-remote` (M2-16), not built:
//! [`LoopbackTransport`] is the labelled in-memory implementation (D-4) — a loopback link
//! whose "other device" a test (or the demo) drives.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use forge_ui::{LiveCell, LiveSource};
use serde::{Deserialize, Serialize};

use crate::EditorError;
use crate::connect::audit::{AuditBook, AuditOrigin};

/// How far the remote host listens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exposure {
    /// Loopback only (the default): nothing off this machine can connect.
    #[default]
    Loopback,
    /// The local network: paired devices on the LAN can connect (an explicit opt-in).
    Lan,
}

impl Exposure {
    /// Its label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Loopback => forge_ui::tr_key!("loopback only (this machine)"),
            Self::Lan => forge_ui::tr_key!("the local network (LAN)"),
        }
    }
}

/// A paired device (user config).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDevice {
    /// Its stable id (`dev-<n>`).
    pub id: String,
    pub name: String,
    /// Who allowed it, here.
    pub allowed_by: String,
    pub paired_at_ms: u64,
    /// The SHA-256 of the device's certificate, lowercase hex (`forge-remote` pins it: only
    /// this device's key opens a session). Empty for a device paired over a transport that
    /// has no certificates (the loopback stand-in).
    #[serde(default)]
    pub fingerprint: String,
    /// What its remote sessions may do, checked in the core's one grant table
    /// (`Principal::Remote(<id>)`, Ch.34 §34.5: the same grant model as automation sessions and
    /// plugins). A new pairing gets [`default_remote_capabilities`]; the destructive class is only
    /// ever added by a person here ([`RemotePairingStore::set_capabilities`], audited).
    #[serde(default = "default_remote_capabilities")]
    pub capabilities: Vec<forge_plugin::Capability>,
}

/// What a newly paired device's sessions may do: read the project and send ordinary
/// (undoable, non-destructive) commands — the capabilities a default policy may pre-grant.
pub fn default_remote_capabilities() -> Vec<forge_plugin::Capability> {
    vec![
        forge_plugin::Capability::Fs(forge_plugin::FsScope::ProjectRead),
        forge_plugin::Capability::Command(forge_plugin::CommandClass::Ordinary),
    ]
}

/// A device asking to pair, with the code its person typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingRequest {
    pub device: String,
    /// The code as the device sent it (the loopback stand-in); empty over a transport that
    /// never sends the code (`forge-remote` proves knowledge of it by a key exchange).
    pub code: String,
    /// The device's certificate fingerprint (empty without certificates).
    pub fingerprint: String,
}

/// Told the pairing file after every change (and once when set): a transport follows the
/// paired devices and the exposure through it.
pub type PairingObserver = Arc<dyn Fn(&PairingFile) + Send + Sync>;

/// A remote editor session (a paired device's client of the core).
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteSession {
    pub id: String,
    pub device: String,
    pub started_at_ms: u64,
    pub commands: u64,
}

/// What pairing and exposure the store holds (the file's content).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingFile {
    pub exposure: Exposure,
    pub devices: Vec<PairedDevice>,
    pub next_device: u64,
}

/// The file the store writes, under the user config directory.
pub const PAIRING_FILE: &str = "remote_pairing.ron";

/// The allow-listed writer of remote pairing and exposure (§21.18; see the module docs).
pub struct RemotePairingStore {
    dir: Option<std::path::PathBuf>,
    file: PairingFile,
    audit: AuditBook,
    revision: u64,
    observer: Option<PairingObserver>,
}

impl std::fmt::Debug for RemotePairingStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemotePairingStore")
            .field("dir", &self.dir)
            .field("file", &self.file)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

impl RemotePairingStore {
    /// The store under `config_dir` (`None`: memory only, nothing kept), auditing to
    /// `audit`. A missing file is an empty store; a bad one is reported and replaced by an
    /// empty store at the next write (never a silent grant: an unreadable file pairs
    /// nothing).
    pub fn open(
        config_dir: Option<&std::path::Path>,
        audit: AuditBook,
    ) -> (Self, Option<EditorError>) {
        let mut err = None;
        let file = match config_dir {
            Some(d) => match crate::user_config::read_optional(&d.join(PAIRING_FILE)) {
                Ok(Some(t)) => ron::from_str(&t).unwrap_or_else(|e| {
                    err = Some(EditorError::Io(format!("{PAIRING_FILE}: {e}")));
                    PairingFile::default()
                }),
                Ok(None) => PairingFile::default(),
                Err(e) => {
                    err = Some(e);
                    PairingFile::default()
                }
            },
            None => PairingFile::default(),
        };
        (
            Self {
                dir: config_dir.map(std::path::Path::to_path_buf),
                file,
                audit,
                revision: 0,
                observer: None,
            },
            err,
        )
    }

    /// Tell `o` the pairing file now and after every change (a transport follows the paired
    /// devices and the exposure; `None` stops).
    pub fn set_observer(&mut self, o: Option<PairingObserver>) {
        if let Some(o) = &o {
            o(&self.file);
        }
        self.observer = o;
    }

    /// The pairing file as it is.
    #[must_use]
    pub fn file(&self) -> &PairingFile {
        &self.file
    }

    fn save(&mut self) -> Result<(), EditorError> {
        self.revision += 1;
        if let Some(o) = &self.observer {
            o(&self.file);
        }
        let Some(d) = &self.dir else { return Ok(()) };
        let text = ron::ser::to_string_pretty(&self.file, ron::ser::PrettyConfig::default())
            .map_err(|e| EditorError::Io(e.to_string()))?;
        crate::user_config::write_atomic(&d.join(PAIRING_FILE), text.as_bytes())
    }

    /// The paired devices.
    #[must_use]
    pub fn devices(&self) -> &[PairedDevice] {
        &self.file.devices
    }

    /// The exposure.
    #[must_use]
    pub fn exposure(&self) -> Exposure {
        self.file.exposure
    }

    /// Changes on every write.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Record a pairing a person allowed (`by`), at `now_ms`.
    pub fn pair(&mut self, name: &str, by: &str, now_ms: u64) -> Result<PairedDevice, EditorError> {
        self.pair_verified(name, None, by, now_ms)
    }

    /// Record a pairing a person allowed (`by`) of a device whose certificate the transport
    /// verified (`fingerprint`, from [`RemoteTransport::take_verified`]). A device already
    /// paired under that fingerprint is paired again under its new name (one record per key).
    pub fn pair_verified(
        &mut self,
        name: &str,
        fingerprint: Option<&str>,
        by: &str,
        now_ms: u64,
    ) -> Result<PairedDevice, EditorError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
            return Err(EditorError::Remote(format!(
                "{name:?} is not a device name"
            )));
        }
        let fingerprint = fingerprint.unwrap_or_default().to_string();
        if !fingerprint.is_empty() {
            self.file.devices.retain(|d| d.fingerprint != fingerprint);
        }
        self.file.next_device += 1;
        let d = PairedDevice {
            id: format!("dev-{}", self.file.next_device),
            name: name.to_string(),
            allowed_by: by.to_string(),
            paired_at_ms: now_ms,
            fingerprint,
            capabilities: default_remote_capabilities(),
        };
        self.file.devices.push(d.clone());
        self.save()?;
        self.audit.record(
            AuditOrigin::Remote,
            &d.id,
            by,
            "paired",
            format!("{} (\u{201c}{}\u{201d}) may connect", d.id, d.name),
        );
        Ok(d)
    }

    /// Set what device `id`'s remote sessions may do (`by` decided it here; audited). The
    /// change reaches a connected session at its next request.
    pub fn set_capabilities(
        &mut self,
        id: &str,
        caps: &[forge_plugin::Capability],
        by: &str,
    ) -> Result<(), EditorError> {
        let Some(d) = self.file.devices.iter_mut().find(|d| d.id == id) else {
            return Err(EditorError::Remote(format!("no paired device {id}")));
        };
        let mut caps = caps.to_vec();
        caps.sort();
        caps.dedup();
        if d.capabilities == caps {
            return Ok(());
        }
        d.capabilities = caps.clone();
        self.save()?;
        self.audit.record(
            AuditOrigin::Remote,
            id,
            by,
            "capabilities",
            format!(
                "{id} may: {}",
                caps.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        Ok(())
    }

    /// Forget a device (`by` unpaired it).
    pub fn unpair(&mut self, id: &str, by: &str) -> Result<(), EditorError> {
        let before = self.file.devices.len();
        self.file.devices.retain(|d| d.id != id);
        if self.file.devices.len() == before {
            return Err(EditorError::Remote(format!("no paired device {id}")));
        }
        self.save()?;
        self.audit.record(
            AuditOrigin::Remote,
            id,
            by,
            "unpaired",
            format!("{id} may no longer connect"),
        );
        Ok(())
    }

    /// Change the exposure. LAN needs `confirmed` (the person said yes to what it means).
    pub fn set_exposure(
        &mut self,
        e: Exposure,
        confirmed: bool,
        by: &str,
    ) -> Result<(), EditorError> {
        if e == Exposure::Lan && !confirmed {
            return Err(EditorError::Remote(
                "LAN exposure lets paired devices on the network connect; it needs an explicit confirmation".into(),
            ));
        }
        if self.file.exposure == e {
            return Ok(());
        }
        self.file.exposure = e;
        self.save()?;
        self.audit.record(
            AuditOrigin::Remote,
            "",
            by,
            "exposure",
            format!("the remote host listens on {}", e.label()),
        );
        Ok(())
    }

    /// Record a denied pairing request (audited, nothing stored).
    pub fn denied(&self, device: &str, by: &str, why: &str) {
        self.audit.record(
            AuditOrigin::Remote,
            "",
            by,
            "pairing_denied",
            format!("\u{201c}{device}\u{201d}: {why}"),
        );
    }

    /// Record a remote session's start or end.
    pub fn session_event(&self, session: &str, device: &str, event: &str) {
        self.audit.record(
            AuditOrigin::Remote,
            session,
            device,
            event,
            format!("{session} from {device}"),
        );
    }
}

/// A pairing code this editor shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingOffer {
    /// Six digits.
    pub code: String,
    /// When it stops working (ms since the Unix epoch).
    pub expires_at_ms: u64,
}

/// How long a pairing code works.
pub const PAIRING_CODE_MS: u64 = 120_000;

/// The link's round-trip latency, as a live feed (see the module docs).
#[derive(Default)]
pub struct LatencyFeed {
    cell: LiveCell,
    micros: AtomicU64,
}

impl std::fmt::Debug for LatencyFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LatencyFeed")
            .field("ms", &self.ms())
            .finish_non_exhaustive()
    }
}

impl LatencyFeed {
    /// The latest round trip, in milliseconds (`None`: nothing measured yet).
    #[must_use]
    pub fn ms(&self) -> Option<f64> {
        match self.micros.load(Ordering::Acquire) {
            0 => None,
            us => Some(us as f64 / 1000.0),
        }
    }
    /// Producer side: a round trip was measured.
    pub fn record(&self, ms: f64) {
        let us = (ms.max(0.001) * 1000.0).round() as u64;
        self.micros.store(us.max(1), Ordering::Release);
        self.cell.bump();
    }
    /// Producer side: the link is up (measuring) or down.
    pub fn set_live(&self, live: bool) {
        self.cell.set_live(live);
    }
}

impl LiveSource for LatencyFeed {
    fn generation(&self) -> u64 {
        self.cell.generation()
    }
    fn is_live(&self) -> bool {
        self.cell.is_live()
    }
    fn set_waker(&self, waker: Option<Arc<dyn forge_ui::UiWaker>>) {
        self.cell.set_waker(waker);
    }
    fn consumed(&self) {
        self.cell.consumed();
    }
}

/// The split editor's transport (Ch.34; see the module docs).
pub trait RemoteTransport {
    /// What it is (a labelled in-memory transport says so).
    fn backend(&self) -> String;
    /// Where the remote host listens, for `exposure`.
    fn address(&self, exposure: Exposure) -> String;
    /// Show a new one-time pairing code (replacing any earlier one).
    fn begin_pairing(&mut self, now_ms: u64) -> PairingOffer;
    /// The pairing code on show, if any.
    fn offer(&self) -> Option<&PairingOffer>;
    /// Devices asking to pair (their person typed a code), oldest first.
    fn requests(&self) -> Vec<PairingRequest>;
    /// Answer a request: `Ok(device name)` if its code is the one on show and unexpired
    /// (the offer is used up), `EDITOR-0012` otherwise. The request is gone either way.
    fn answer(&mut self, device: &str, now_ms: u64) -> Result<String, EditorError>;
    /// A person refuses `device`'s request: the device is told `why` (the real reason, not a
    /// code problem it did not have), and the request is gone. The pairing code **stays on
    /// show** — another device may still type it; showing a new one or letting it expire is
    /// the person's call. `EDITOR-0012` if no request from `device` is waiting.
    fn deny(&mut self, device: &str, why: &str) -> Result<(), EditorError>;
    /// After a successful [`RemoteTransport::answer`]: the certificate fingerprint the
    /// transport verified for `device`, for the pairing record
    /// ([`RemotePairingStore::pair_verified`]). `None` without certificates.
    fn take_verified(&mut self, _device: &str) -> Option<String> {
        None
    }
    /// The remote sessions connected now.
    fn sessions(&self) -> Vec<RemoteSession>;
    /// End a session.
    fn disconnect(&mut self, session: &str) -> bool;
    /// The link's latency feed.
    fn latency(&self) -> Arc<LatencyFeed>;
    /// Changes whenever requests or sessions change.
    fn revision(&self) -> u64;
    /// A live source bumped whenever requests or sessions change (a device asks to pair
    /// from another thread): the panel refreshes on it, at most twice a second.
    fn changes(&self) -> Arc<dyn LiveSource>;
}

/// The labelled in-memory loopback transport (D-4 until `forge-remote`, M2-16).
pub struct LoopbackTransport {
    offer: Option<PairingOffer>,
    requests: Vec<PairingRequest>,
    sessions: Vec<RemoteSession>,
    latency: Arc<LatencyFeed>,
    changes: Arc<LiveCell>,
    next_session: u64,
    /// Codes come from this counter mixed with the offer time (deterministic in tests;
    /// the real transport draws them from the OS's random source).
    seed: u64,
    revision: u64,
    port: u16,
    /// W2 positive control only: pair whatever code a device typed
    /// (`test_remote_connect`'s control). Never set outside that test.
    #[doc(hidden)]
    pub accept_any_code: bool,
}

impl std::fmt::Debug for LoopbackTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopbackTransport")
            .field("requests", &self.requests.len())
            .field("sessions", &self.sessions.len())
            .finish_non_exhaustive()
    }
}

impl Default for LoopbackTransport {
    fn default() -> Self {
        Self::new(0x5EED_C0DE)
    }
}

impl LoopbackTransport {
    /// A loopback transport whose pairing codes derive from `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            offer: None,
            requests: Vec::new(),
            sessions: Vec::new(),
            latency: Arc::new(LatencyFeed::default()),
            changes: LiveCell::new(),
            next_session: 1,
            seed,
            revision: 0,
            port: 47_900,
            accept_any_code: false,
        }
    }

    fn touched(&mut self) {
        self.revision += 1;
        self.changes.bump();
    }

    /// **The other device** (a test, the demo): its person typed `code` and it asks to pair.
    pub fn device_requests(&mut self, device: &str, code: &str) {
        self.requests.retain(|r| r.device != device);
        self.requests.push(PairingRequest {
            device: device.to_string(),
            code: code.to_string(),
            fingerprint: String::new(),
        });
        self.touched();
    }

    /// **The other device** connects as a paired device: a remote session starts.
    pub fn device_connects(&mut self, device: &str, now_ms: u64) -> String {
        let id = format!("remote-{}", self.next_session);
        self.next_session += 1;
        self.sessions.push(RemoteSession {
            id: id.clone(),
            device: device.to_string(),
            started_at_ms: now_ms,
            commands: 0,
        });
        self.latency.set_live(true);
        self.touched();
        id
    }
}

fn code_from(seed: u64, n: u64) -> String {
    // SplitMix64: a six-digit code that differs per offer.
    let mut z = seed.wrapping_add(n.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    format!("{:06}", z % 1_000_000)
}

impl RemoteTransport for LoopbackTransport {
    fn backend(&self) -> String {
        "in-memory loopback transport (D-4: the split editor's QUIC transport is forge-remote, M2-16, UNBUILT)".into()
    }
    fn address(&self, exposure: Exposure) -> String {
        match exposure {
            Exposure::Loopback => format!("127.0.0.1:{}", self.port),
            Exposure::Lan => format!("0.0.0.0:{} (every LAN interface)", self.port),
        }
    }
    fn begin_pairing(&mut self, now_ms: u64) -> PairingOffer {
        self.seed = self.seed.wrapping_add(1);
        let o = PairingOffer {
            code: code_from(self.seed, now_ms),
            expires_at_ms: now_ms + PAIRING_CODE_MS,
        };
        self.offer = Some(o.clone());
        self.touched();
        o
    }
    fn offer(&self) -> Option<&PairingOffer> {
        self.offer.as_ref()
    }
    fn requests(&self) -> Vec<PairingRequest> {
        self.requests.clone()
    }
    fn answer(&mut self, device: &str, now_ms: u64) -> Result<String, EditorError> {
        let Some(pos) = self.requests.iter().position(|r| r.device == device) else {
            return Err(EditorError::Remote(format!(
                "no pairing request from {device:?}"
            )));
        };
        let r = self.requests.remove(pos);
        self.touched();
        match &self.offer {
            None => Err(EditorError::Remote(
                "no pairing code is on show: show one, then type it on the other device".into(),
            )),
            Some(o) if now_ms > o.expires_at_ms => {
                self.offer = None;
                Err(EditorError::Remote(
                    "the pairing code expired; show a new one".into(),
                ))
            }
            Some(o) if o.code != r.code.trim() && !self.accept_any_code => {
                Err(EditorError::Remote(format!(
                    "{device:?} typed a code that is not the one on show"
                )))
            }
            Some(_) => {
                self.offer = None;
                Ok(r.device)
            }
        }
    }
    fn deny(&mut self, device: &str, why: &str) -> Result<(), EditorError> {
        let Some(pos) = self.requests.iter().position(|r| r.device == device) else {
            return Err(EditorError::Remote(format!(
                "no pairing request from {device:?}"
            )));
        };
        // In memory there is no device to tell; the request goes, the offer stays.
        let _ = why;
        self.requests.remove(pos);
        self.touched();
        Ok(())
    }
    fn sessions(&self) -> Vec<RemoteSession> {
        self.sessions.clone()
    }
    fn disconnect(&mut self, session: &str) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|s| s.id != session);
        if self.sessions.is_empty() {
            self.latency.set_live(false);
        }
        self.touched();
        self.sessions.len() != before
    }
    fn latency(&self) -> Arc<LatencyFeed> {
        Arc::clone(&self.latency)
    }
    fn revision(&self) -> u64 {
        self.revision
    }
    fn changes(&self) -> Arc<dyn LiveSource> {
        self.changes.clone() as Arc<dyn LiveSource>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_pairs_once_and_a_wrong_or_late_one_never() {
        let mut t = LoopbackTransport::new(1);
        let o = t.begin_pairing(1_000);
        assert_eq!(o.code.len(), 6);
        t.device_requests("laptop", "000000");
        assert!(t.answer("laptop", 2_000).is_err(), "a wrong code");
        t.device_requests("laptop", &o.code);
        assert_eq!(t.answer("laptop", 2_000).ok().as_deref(), Some("laptop"));
        t.device_requests("tablet", &o.code);
        assert!(t.answer("tablet", 2_000).is_err(), "the code is used up");
        let o = t.begin_pairing(10_000);
        t.device_requests("tablet", &o.code);
        assert!(
            t.answer("tablet", 10_000 + PAIRING_CODE_MS + 1).is_err(),
            "expired"
        );
    }

    #[test]
    fn lan_needs_confirmation_and_every_change_is_audited() {
        let book = AuditBook::new(Box::new(forge_cmd::FixedClock(9)));
        let (mut s, e) = RemotePairingStore::open(None, book.clone());
        assert!(e.is_none());
        assert_eq!(s.exposure(), Exposure::Loopback);
        assert!(s.set_exposure(Exposure::Lan, false, "ada").is_err());
        assert_eq!(s.exposure(), Exposure::Loopback);
        s.set_exposure(Exposure::Lan, true, "ada")
            .unwrap_or_else(|e| panic!("{e}"));
        let d = s.pair("laptop", "ada", 5).unwrap_or_else(|e| panic!("{e}"));
        s.unpair(&d.id, "ada").unwrap_or_else(|e| panic!("{e}"));
        let events: Vec<String> = book
            .query(&crate::connect::audit::AuditQuery::default())
            .into_iter()
            .map(|r| r.event)
            .collect();
        assert_eq!(events, ["exposure", "paired", "unpaired"]);
    }
}
