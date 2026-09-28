//! The host: the editor process that owns the core serves it to paired devices over QUIC —
//! [`QuicTransport`], the real [`RemoteTransport`] behind the Remote panel (WP-U9's connect /
//! pair dialog, session list and latency indicator).
//!
//! * **Loopback by default.** The endpoint binds `127.0.0.1`; the pairing store's LAN
//!   exposure (an explicit, confirmed, audited choice) rebinds it to every interface on the
//!   same port, and back. Nothing off this machine can reach a loopback socket.
//! * **Pairing** ([`crate::pairing`]): a device's request is listed for a person here only
//!   after the key exchange over the code on show; [`RemoteTransport::answer`] refuses an
//!   unverified one exactly as the stand-in refused a wrong code, and a code is withdrawn
//!   after [`crate::pairing::MAX_ATTEMPTS`] failed exchanges.
//! * **Paired devices** come from the pairing store through its observer
//!   ([`QuicTransport::attach`]): the store stays the one writer of pairing (user config,
//!   audited). A device's capabilities are mirrored into the core's **one grant table** as
//!   `Principal::Remote(<device id>)`; unpairing revokes them and closes its sessions.
//! * **Sessions** ([`crate::session`]): a paired device's connection becomes a client of
//!   the core through the core's own `LocalBus`, issued as the person who paired it at that
//!   device. Every request is checked against the grant table before it reaches the bus,
//!   and then the bus's own policies apply. Each session and each denial is audited.

use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use forge_editor::EditorError;
use forge_editor::connect::audit::{AuditBook, AuditOrigin};
use forge_editor::connect::remote::{
    Exposure, LatencyFeed, PAIRING_CODE_MS, PairedDevice, PairingFile, PairingObserver,
    PairingOffer, PairingRequest, RemoteSession, RemoteTransport,
};
use forge_editor::core::{EditorCore, SharedCore};
use forge_plugin::{Capability, Principal, SharedGrants};
use forge_ui::{LiveCell, LiveSource};
use tokio::sync::oneshot;

use crate::identity::{self, HOST_NAME, Identity};
use crate::pairing::{self, Exchange, MAX_ATTEMPTS, Side};
use crate::wire::{self, DeviceConfirm, Hello, HostMsg, PairMsg};
use crate::{RemoteError, session};

/// Where the host keeps its identity, under the user config directory.
pub const HOST_IDENTITY_FILE: &str = "remote_host_identity.ron";

forge_trace::control_switches! {
    /// W2 positive-control switches for the host's guards. Never set outside those tests.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct HostFaults {
        /// A device whose key confirmation failed (a wrong code) is listed as verified.
        pub skip_code_check: bool,
        /// Remote requests reach the bus without the grant check.
        pub skip_grant_check: bool,
        /// The host sends an empty batch to every session twice a second (a heartbeat on the
        /// session stream): an idle client then wakes (`test_split_editor_idle`'s control).
        pub heartbeat: bool,
    }
}

/// How to start the host.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// The UDP port (0: any free one).
    pub port: u16,
    /// The exposure to start with (the pairing store's, once attached).
    pub exposure: Exposure,
    /// The host's identity (`None`: the one kept under `config_dir`, made on first use).
    pub identity: Option<Identity>,
    /// The user config directory (`None`: a fresh identity kept nowhere).
    pub config_dir: Option<PathBuf>,
    /// A simulated one-way delay on everything the host sends (tests of the split editor
    /// under a slow link; zero in the editor).
    #[doc(hidden)]
    pub simulated_delay: Duration,
    /// Where LAN exposure listens: every interface (`0.0.0.0`). Tests use a second loopback
    /// address (`127.0.0.2`), which exercises the rebind without opening a port to the
    /// network.
    #[doc(hidden)]
    pub lan_ip: Ipv4Addr,
    #[doc(hidden)]
    pub faults: HostFaults,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            port: crate::DEFAULT_PORT,
            exposure: Exposure::Loopback,
            identity: None,
            config_dir: None,
            simulated_delay: Duration::ZERO,
            lan_ip: Ipv4Addr::UNSPECIFIED,
            faults: HostFaults::default(),
        }
    }
}

struct Pending {
    req: PairingRequest,
    verified: bool,
    reply: Option<oneshot::Sender<PairMsg>>,
}

pub(crate) struct Live {
    pub(crate) info: RemoteSession,
    pub(crate) device_id: String,
    pub(crate) conn: quinn::Connection,
}

#[derive(Default)]
pub(crate) struct HostState {
    offer: Option<PairingOffer>,
    failed_attempts: u32,
    requests: Vec<Pending>,
    pub(crate) paired: Vec<PairedDevice>,
    exposure: Exposure,
    pub(crate) sessions: Vec<Live>,
    /// Answered pairings, until the panel takes the fingerprint for the store.
    verified: HashMap<String, String>,
    /// Allowed devices waiting for the store to record them: `(fingerprint, reply)`.
    awaiting: Vec<(String, oneshot::Sender<PairMsg>)>,
    /// What the grant table holds for each paired device (to revoke what is gone).
    granted: BTreeMap<String, Vec<Capability>>,
    pub(crate) next_session: u64,
    revision: u64,
}

/// What the network tasks and the transport share.
pub(crate) struct Shared {
    state: Mutex<HostState>,
    changes: Arc<LiveCell>,
    pub(crate) latency: Arc<LatencyFeed>,
    pub(crate) core: SharedCore,
    pub(crate) grants: SharedGrants,
    pub(crate) audit: AuditBook,
    endpoint: quinn::Endpoint,
    rt: tokio::runtime::Handle,
    pub(crate) faults: HostFaults,
    pub(crate) delay: Duration,
    lan_ip: Ipv4Addr,
    fingerprint: String,
}

impl Shared {
    pub(crate) fn lock(&self) -> MutexGuard<'_, HostState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Requests or sessions changed: the panel refreshes (from any thread).
    pub(crate) fn touched(&self, st: &mut HostState) {
        st.revision += 1;
        self.changes.bump();
    }
}

/// The split editor's QUIC transport (see the module docs).
pub struct QuicTransport {
    shared: Arc<Shared>,
    runtime: Option<tokio::runtime::Runtime>,
    offer: Option<PairingOffer>,
}

impl std::fmt::Debug for QuicTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicTransport")
            .field("addr", &self.local_addr())
            .field("fingerprint", &self.shared.fingerprint)
            .finish_non_exhaustive()
    }
}

fn bind_addr(exposure: Exposure, port: u16, lan: Ipv4Addr) -> SocketAddr {
    let ip = match exposure {
        Exposure::Loopback => Ipv4Addr::LOCALHOST,
        Exposure::Lan => lan,
    };
    SocketAddr::V4(SocketAddrV4::new(ip, port))
}

/// Move the endpoint to `to` on the same port. If the port cannot be bound while the old
/// socket still holds it (some stacks refuse a wildcard beside a specific address), step
/// through a temporary loopback socket to free it first.
fn rebind(s: &Shared, to: SocketAddr) -> std::io::Result<()> {
    let _g = s.rt.enter();
    match UdpSocket::bind(to) {
        Ok(sock) => s.endpoint.rebind(sock),
        Err(_) => {
            let tmp = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))?;
            s.endpoint.rebind(tmp)?;
            s.endpoint.rebind(UdpSocket::bind(to)?)
        }
    }
}

impl QuicTransport {
    /// Serve `core` (see the module docs). The endpoint listens at once; no device is
    /// admitted until the pairing store is [`QuicTransport::attach`]ed.
    pub fn start(core: SharedCore, cfg: HostConfig) -> Result<Self, RemoteError> {
        let identity = match cfg.identity {
            Some(i) => i,
            None => {
                Identity::load_or_create(cfg.config_dir.as_deref(), HOST_IDENTITY_FILE, HOST_NAME)?
            }
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("forge-remote-host")
            .enable_all()
            .build()
            .map_err(|e| RemoteError::Transport(e.to_string()))?;
        let endpoint = {
            let _g = runtime.enter();
            quinn::Endpoint::server(
                identity::server_config(&identity)?,
                bind_addr(cfg.exposure, cfg.port, cfg.lan_ip),
            )
            .map_err(|e| RemoteError::Transport(format!("could not listen: {e}")))?
        };
        let latency = Arc::new(LatencyFeed::default());
        let shared = Arc::new(Shared {
            state: Mutex::new(HostState {
                exposure: cfg.exposure,
                next_session: 1,
                ..HostState::default()
            }),
            changes: LiveCell::new(),
            latency,
            grants: EditorCore::grants(&core),
            audit: EditorCore::audit_book(&core),
            core,
            endpoint,
            rt: runtime.handle().clone(),
            faults: cfg.faults,
            delay: cfg.simulated_delay,
            lan_ip: cfg.lan_ip,
            fingerprint: identity.fingerprint().to_string(),
        });
        runtime.spawn(accept_loop(Arc::clone(&shared)));
        Ok(Self {
            shared,
            runtime: Some(runtime),
            offer: None,
        })
    }

    /// Follow `store`: its paired devices (admitted, their capabilities in the grant table)
    /// and its exposure (the endpoint rebinds).
    pub fn attach(&self, store: &mut forge_editor::connect::remote::RemotePairingStore) {
        store.set_observer(Some(self.observer()));
    }

    /// The observer [`QuicTransport::attach`] installs.
    #[must_use]
    pub fn observer(&self) -> PairingObserver {
        let shared = Arc::downgrade(&self.shared);
        Arc::new(move |file: &PairingFile| {
            if let Some(s) = shared.upgrade() {
                follow_pairing(&s, file);
            }
        })
    }

    /// Where the endpoint listens now.
    #[must_use]
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.shared.endpoint.local_addr().ok()
    }

    /// The host certificate's fingerprint (what a paired device pins).
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.shared.fingerprint
    }

    /// The pairing code on show right now, as the host checks it (a code is withdrawn after
    /// too many failed exchanges).
    #[must_use]
    pub fn live_offer(&self) -> Option<PairingOffer> {
        self.shared.lock().offer.clone()
    }
}

impl Drop for QuicTransport {
    fn drop(&mut self) {
        self.shared
            .endpoint
            .close(0u32.into(), b"the editor is closing");
        for l in &self.shared.lock().sessions {
            l.conn.close(0u32.into(), b"the editor is closing");
        }
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

/// The store changed (or was attached): follow it.
fn follow_pairing(s: &Arc<Shared>, file: &PairingFile) {
    let mut st = s.lock();
    st.paired = file.devices.clone();
    // Allowed devices the store has now recorded learn their id.
    let awaiting = std::mem::take(&mut st.awaiting);
    for (fp, tx) in awaiting {
        match st.paired.iter().find(|d| d.fingerprint == fp) {
            Some(d) => {
                let _ = tx.send(PairMsg::Paired {
                    device_id: d.id.clone(),
                });
            }
            None => st.awaiting.push((fp, tx)),
        }
    }
    // The grant table follows each device's capabilities; an unpaired device loses them.
    let mut want: BTreeMap<String, Vec<Capability>> = BTreeMap::new();
    for d in &st.paired {
        want.insert(d.id.clone(), d.capabilities.clone());
    }
    for (id, caps) in &st.granted {
        let keep = want.get(id);
        for c in caps {
            if keep.is_none_or(|k| !k.contains(c)) {
                s.grants.revoke(&Principal::Remote(id.clone()), *c);
            }
        }
    }
    for (id, caps) in &want {
        for c in caps {
            s.grants.grant(Principal::Remote(id.clone()), *c);
        }
    }
    st.granted = want;
    // Sessions of a device no longer paired end now.
    let paired: Vec<String> = st.paired.iter().map(|d| d.id.clone()).collect();
    st.sessions.retain(|l| {
        let keep = paired.contains(&l.device_id);
        if !keep {
            l.conn.close(1u32.into(), b"this device was unpaired");
        }
        keep
    });
    // Exposure: rebind on the same port.
    if st.exposure != file.exposure {
        let port = s.endpoint.local_addr().map_or(0, |a| a.port());
        let rebound = rebind(s, bind_addr(file.exposure, port, s.lan_ip));
        match rebound {
            Ok(()) => st.exposure = file.exposure,
            Err(e) => s.audit.record(
                AuditOrigin::Remote,
                "",
                "",
                "exposure_failed",
                format!("could not listen on {}: {e}", file.exposure.label()),
            ),
        }
    }
    s.touched(&mut st);
}

async fn accept_loop(s: Arc<Shared>) {
    while let Some(incoming) = s.endpoint.accept().await {
        let s = Arc::clone(&s);
        tokio::spawn(async move {
            let _ = connection(s, incoming).await;
        });
    }
}

async fn connection(s: Arc<Shared>, incoming: quinn::Incoming) -> Result<(), RemoteError> {
    let conn = incoming
        .await
        .map_err(|e| RemoteError::Transport(e.to_string()))?;
    let Some(fp) = identity::peer_fingerprint(&conn) else {
        conn.close(2u32.into(), b"a client certificate is required");
        return Err(RemoteError::Tls("no client certificate".into()));
    };
    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .map_err(|e| RemoteError::Transport(e.to_string()))?;
    let hello = tokio::time::timeout(Duration::from_secs(10), wire::recv::<Hello>(&mut recv))
        .await
        .map_err(|_| RemoteError::Timeout("the device said nothing".into()))??;
    match hello {
        Some(Hello::Pair { name, spake }) => {
            pair(&s, &mut send, &mut recv, name, &spake, &fp).await?;
            finish(&mut send).await;
            Ok(())
        }
        Some(Hello::Session { follow_session }) => {
            let device = s
                .lock()
                .paired
                .iter()
                .find(|d| d.fingerprint == fp)
                .cloned();
            match device {
                None => {
                    s.audit.record(
                        AuditOrigin::Remote,
                        "",
                        &identity::short(&fp),
                        "session_refused",
                        format!(
                            "a device with certificate {} is not paired",
                            identity::short(&fp)
                        ),
                    );
                    let _ = wire::send(
                        &mut send,
                        &HostMsg::Closed {
                            why: "this device is not paired with this editor: pair it first (Remote \u{203a} Pair a device)".into(),
                        },
                    )
                    .await;
                    finish(&mut send).await;
                    conn.close(3u32.into(), b"not paired");
                    Ok(())
                }
                Some(d) => session::run(s, conn, send, recv, d, follow_session).await,
            }
        }
        None => Ok(()),
    }
}

/// Finish a stream and give the peer a moment to read it before the connection goes.
async fn finish(send: &mut quinn::SendStream) {
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(2), send.stopped()).await;
}

async fn pair(
    s: &Arc<Shared>,
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    name: String,
    spake: &[u8],
    device_fp: &str,
) -> Result<(), RemoteError> {
    let now = s.audit.now_ms();
    let code = {
        let st = s.lock();
        st.offer
            .as_ref()
            .filter(|o| now <= o.expires_at_ms)
            .map(|o| o.code.clone())
    };
    let Some(code) = code else {
        return wire::send(
            send,
            &PairMsg::Refused {
                why: "no pairing code is on show on the host: show one there (Remote \u{203a} Pair a device), then type it here".into(),
            },
        )
        .await;
    };
    let (ex, msg) = Exchange::host(&code);
    let key = match ex.finish(spake) {
        Ok(k) => k,
        Err(e) => {
            return wire::send(send, &PairMsg::Refused { why: e.to_string() }).await;
        }
    };
    let host_fp = s.fingerprint.clone();
    wire::send(
        send,
        &PairMsg::Spake {
            spake: msg,
            confirm: pairing::confirm(&key, Side::Host, &host_fp, device_fp),
        },
    )
    .await?;
    let dc = tokio::time::timeout(Duration::from_secs(30), wire::recv::<DeviceConfirm>(recv))
        .await
        .map_err(|_| RemoteError::Timeout("the device did not confirm the key".into()))??;
    let ok =
        dc.is_some_and(|c| pairing::check(&key, Side::Device, &host_fp, device_fp, &c.confirm));
    let verified = ok || s.faults.skip_code_check();
    let (tx, rx) = oneshot::channel();
    {
        let mut st = s.lock();
        if !ok {
            st.failed_attempts += 1;
            if st.failed_attempts >= MAX_ATTEMPTS {
                st.offer = None;
                s.audit.record(
                    AuditOrigin::Remote,
                    "",
                    "",
                    "pairing_code_withdrawn",
                    format!("{MAX_ATTEMPTS} exchanges failed on the code on show"),
                );
            }
        }
        st.requests.retain(|p| p.req.device != name);
        st.requests.push(Pending {
            req: PairingRequest {
                device: name,
                code: String::new(),
                fingerprint: device_fp.to_string(),
            },
            verified,
            reply: Some(tx),
        });
        s.touched(&mut st);
    }
    wire::send(send, &PairMsg::Waiting).await?;
    // A person answers here; the device waits for them (bounded).
    let answer = tokio::time::timeout(
        Duration::from_millis(PAIRING_CODE_MS) + Duration::from_secs(60),
        rx,
    )
    .await;
    let msg = match answer {
        Ok(Ok(m)) => m,
        _ => PairMsg::Denied {
            why: "no one answered the request on the host in time".into(),
        },
    };
    wire::send(send, &msg).await
}

impl RemoteTransport for QuicTransport {
    fn backend(&self) -> String {
        format!(
            "QUIC over TLS 1.3 (forge-remote), host certificate {}",
            identity::short(&self.shared.fingerprint)
        )
    }

    fn address(&self, exposure: Exposure) -> String {
        let port = self.local_addr().map_or(0, |a| a.port());
        match exposure {
            Exposure::Loopback => format!("127.0.0.1:{port}"),
            Exposure::Lan if self.shared.lan_ip.is_unspecified() => {
                format!("0.0.0.0:{port} (every LAN interface)")
            }
            Exposure::Lan => format!("{}:{port}", self.shared.lan_ip),
        }
    }

    fn begin_pairing(&mut self, now_ms: u64) -> PairingOffer {
        // The OS's random source; should it ever fail, no code is shown that could pair.
        let code = pairing::fresh_code().unwrap_or_default();
        let o = PairingOffer {
            code,
            expires_at_ms: now_ms + PAIRING_CODE_MS,
        };
        {
            let mut st = self.shared.lock();
            st.offer = (!o.code.is_empty()).then(|| o.clone());
            st.failed_attempts = 0;
            self.shared.touched(&mut st);
        }
        self.offer = Some(o.clone());
        o
    }

    fn offer(&self) -> Option<&PairingOffer> {
        self.offer.as_ref()
    }

    fn requests(&self) -> Vec<PairingRequest> {
        self.shared
            .lock()
            .requests
            .iter()
            .map(|p| p.req.clone())
            .collect()
    }

    fn answer(&mut self, device: &str, now_ms: u64) -> Result<String, EditorError> {
        let mut st = self.shared.lock();
        let Some(pos) = st.requests.iter().position(|p| p.req.device == device) else {
            return Err(EditorError::Remote(format!(
                "no pairing request from {device:?}"
            )));
        };
        let mut p = st.requests.remove(pos);
        self.shared.touched(&mut st);
        let deny = |p: &mut Pending, why: String| {
            if let Some(tx) = p.reply.take() {
                let _ = tx.send(PairMsg::Denied { why: why.clone() });
            }
            Err(EditorError::Remote(why))
        };
        match st.offer.clone() {
            None => deny(
                &mut p,
                "no pairing code is on show: show one, then type it on the other device".into(),
            ),
            Some(o) if now_ms > o.expires_at_ms => {
                st.offer = None;
                self.offer = None;
                deny(&mut p, "the pairing code expired; show a new one".into())
            }
            Some(_) if !p.verified => deny(
                &mut p,
                format!("{device:?} typed a code that is not the one on show"),
            ),
            Some(_) => {
                st.offer = None;
                self.offer = None;
                st.verified
                    .insert(p.req.device.clone(), p.req.fingerprint.clone());
                if let Some(tx) = p.reply.take() {
                    st.awaiting.push((p.req.fingerprint.clone(), tx));
                }
                Ok(p.req.device)
            }
        }
    }

    fn deny(&mut self, device: &str, why: &str) -> Result<(), EditorError> {
        let mut st = self.shared.lock();
        let Some(pos) = st.requests.iter().position(|p| p.req.device == device) else {
            return Err(EditorError::Remote(format!(
                "no pairing request from {device:?}"
            )));
        };
        let mut p = st.requests.remove(pos);
        self.shared.touched(&mut st);
        // The device is told the person's reason; the code on show is left alone.
        if let Some(tx) = p.reply.take() {
            let _ = tx.send(PairMsg::Denied { why: why.into() });
        }
        Ok(())
    }

    fn take_verified(&mut self, device: &str) -> Option<String> {
        self.shared.lock().verified.remove(device)
    }

    fn sessions(&self) -> Vec<RemoteSession> {
        self.shared
            .lock()
            .sessions
            .iter()
            .map(|l| l.info.clone())
            .collect()
    }

    fn disconnect(&mut self, session: &str) -> bool {
        let mut st = self.shared.lock();
        let before = st.sessions.len();
        st.sessions.retain(|l| {
            let keep = l.info.id != session;
            if !keep {
                l.conn.close(4u32.into(), b"disconnected by the host");
            }
            keep
        });
        if st.sessions.is_empty() {
            self.shared.latency.set_live(false);
        }
        self.shared.touched(&mut st);
        st.sessions.len() != before
    }

    fn latency(&self) -> Arc<LatencyFeed> {
        Arc::clone(&self.shared.latency)
    }

    fn revision(&self) -> u64 {
        self.shared.lock().revision
    }

    fn changes(&self) -> Arc<dyn LiveSource> {
        self.shared.changes.clone() as Arc<dyn LiveSource>
    }
}
