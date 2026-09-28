//! [`RemoteBus`] — the split editor's client: a [`BusClient`] whose core is on another
//! machine. The shell and its panels run on it unchanged (Ch.34 §34.2: "panels cannot tell
//! local from remote").
//!
//! * **Nothing on the input path waits for the network.** A request is queued for the
//!   connection's writer and answered through [`BusClient::pump`], exactly as the trait was
//!   written for (ADR 0018: "requests return a ticket, never a result"). A transaction the
//!   client opens is named by an alias until the host says its id.
//! * **Optimistic preview (O-13).** Each command is planned locally with the editor's own
//!   planners over a replica of the project ([`Predictor`]); the diff goes into the next
//!   pump as a [`Prediction`] the mirror shows at once, and the host's answer settles it: a
//!   right prediction simply stays, a wrong one is replaced by what the core did the pump
//!   the answer arrives in.
//! * Reads the trait answers synchronously (`snapshot`, `history`, `txn_info`, the undo
//!   targets, the project status) come from state the host streams; only a dry run of a
//!   command this client cannot plan (a plugin command only the core has) asks the host and
//!   waits, bounded.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use forge_cmd::{
    Applied, CmdError, Diff, EditorCommand, Gap, Issuer, Project, Rejection, TxnId, TxnState,
};
use forge_editor::client::{BusClient, Prediction, Pumped, Refused, Ticket, TxnInfo};
use forge_editor::core::{Predictor, Waker};
use forge_project::status::ProjectStatus;
use tokio::sync::mpsc;

use crate::RemoteError;
use crate::device::PairedHost;
use crate::identity::{self, HOST_NAME, Identity};
use crate::wire::{self, ALIAS_BIT, Hello, HostMsg, Request, WireFaults};

/// How to connect.
#[derive(Clone, Copy, Debug)]
pub struct ConnectOptions {
    /// Receive session commands (the play controls): what a shell hosting a play core
    /// wants (it asks again through [`BusClient::follow_session`] anyway).
    pub follow_session: bool,
    /// How long to wait for the host to answer the connection.
    pub timeout: Duration,
    /// How long a dry run the client cannot plan itself may wait for the host.
    pub preview_timeout: Duration,
    #[doc(hidden)]
    pub faults: WireFaults,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            follow_session: false,
            timeout: Duration::from_secs(10),
            preview_timeout: Duration::from_secs(2),
            faults: WireFaults::default(),
        }
    }
}

#[derive(Default)]
struct Inbox {
    msgs: VecDeque<HostMsg>,
    /// Requests the host has answered (a settled ticket, a `Begun`).
    answered: u64,
    previews: HashMap<u64, Result<Diff, Rejection>>,
    closed: Option<String>,
}

#[derive(Default)]
struct ClientShared {
    inbox: Mutex<Inbox>,
    cv: Condvar,
    waker: Mutex<Option<Waker>>,
}

impl ClientShared {
    fn lock(&self) -> MutexGuard<'_, Inbox> {
        self.inbox.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn wake(&self) {
        self.cv.notify_all();
        let w = self
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(w) = w {
            w();
        }
    }
}

/// A handle on a [`RemoteBus`] that stays usable after the client is boxed into a shell
/// (tests and tools wait on it).
#[derive(Clone)]
pub struct RemoteHandle {
    shared: Arc<ClientShared>,
    expected: Arc<AtomicU64>,
    conn: quinn::Connection,
}

impl std::fmt::Debug for RemoteHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteHandle")
            .field("expected", &self.expected.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl RemoteHandle {
    /// Wait until the host has answered every request sent so far (their answers are then
    /// in the next pump). False on timeout or a closed connection.
    pub fn wait_settled(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        let mut g = self.shared.lock();
        loop {
            if g.answered >= self.expected.load(Ordering::Acquire) {
                return true;
            }
            if g.closed.is_some() {
                return false;
            }
            let now = Instant::now();
            if now >= end {
                return false;
            }
            g = self
                .shared
                .cv
                .wait_timeout(g, end - now)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// Why the host closed the session, if it did.
    #[must_use]
    pub fn closed(&self) -> Option<String> {
        self.shared.lock().closed.clone()
    }

    /// The connection's round-trip time as QUIC measures it.
    #[must_use]
    pub fn rtt(&self) -> Duration {
        self.conn.rtt()
    }

    /// Frames sent and received so far, as the QUIC stack counts them (bytes on the wire,
    /// for the budget measurements).
    #[must_use]
    pub fn bytes(&self) -> (u64, u64) {
        let s = self.conn.stats();
        (s.udp_tx.bytes, s.udp_rx.bytes)
    }
}

/// How long dropping a [`RemoteBus`] waits at most for its close to leave.
const CLOSE_GRACE: Duration = Duration::from_millis(250);

/// See the module docs.
pub struct RemoteBus {
    runtime: Option<tokio::runtime::Runtime>,
    /// Kept to let the close reach the host when the session is dropped.
    endpoint: quinn::Endpoint,
    conn: quinn::Connection,
    out: mpsc::UnboundedSender<Request>,
    shared: Arc<ClientShared>,
    expected: Arc<AtomicU64>,
    issuer: Issuer,
    session: String,
    device_id: String,
    predictor: Predictor,
    infos: HashMap<TxnId, TxnInfo>,
    order: Vec<TxnId>,
    undo: Option<TxnId>,
    redo: Option<TxnId>,
    status: Option<Arc<ProjectStatus>>,
    status_fresh: bool,
    aliases: HashMap<u64, TxnId>,
    predicted: Vec<Prediction>,
    refused_here: Vec<Refused>,
    next_ticket: u64,
    next_alias: u64,
    next_req: std::cell::Cell<u64>,
    closed_reported: bool,
    opts: ConnectOptions,
}

impl std::fmt::Debug for RemoteBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteBus")
            .field("session", &self.session)
            .field("issuer", &self.issuer)
            .field("predictor", &self.predictor)
            .finish_non_exhaustive()
    }
}

impl RemoteBus {
    /// Open a session on `host` (paired) as `identity`'s device.
    pub fn connect(
        identity: &Identity,
        host: &PairedHost,
        opts: ConnectOptions,
    ) -> Result<Self, RemoteError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("forge-remote-client")
            .enable_all()
            .build()
            .map_err(|e| RemoteError::Transport(e.to_string()))?;
        let (cfg, _) = identity::client_config(identity, Some(&host.fingerprint))?;
        let addr = host.addr;
        let timeout = opts.timeout;
        let follow = opts.follow_session;
        let (endpoint, conn, send, mut recv, welcome) = runtime.block_on(async move {
            tokio::time::timeout(timeout, open(cfg, addr, follow))
                .await
                .map_err(|_| RemoteError::Timeout(format!("{addr} did not answer")))?
        })?;
        let welcome = match welcome {
            HostMsg::Welcome(w) => *w,
            HostMsg::Closed { why } => return Err(RemoteError::Refused(why)),
            other => {
                return Err(RemoteError::Protocol(format!(
                    "expected the session's welcome, got {other:?}"
                )));
            }
        };
        let shared = Arc::new(ClientShared::default());
        let (out, mut outbox) = mpsc::unbounded_channel::<Request>();
        let mut send = send;
        runtime.spawn(async move {
            while let Some(r) = outbox.recv().await {
                if wire::send(&mut send, &r).await.is_err() {
                    break;
                }
            }
            let _ = send.finish();
        });
        let reader = Arc::clone(&shared);
        runtime.spawn(async move {
            let why = loop {
                match wire::recv::<HostMsg>(&mut recv).await {
                    Ok(Some(m)) => {
                        let mut g = reader.lock();
                        match m {
                            HostMsg::Preview { req, result } => {
                                g.previews.insert(req, result);
                            }
                            HostMsg::Closed { why } => {
                                g.closed = Some(why);
                            }
                            other => {
                                g.answered += match &other {
                                    HostMsg::Batch(b) => b.settled.len() as u64,
                                    HostMsg::Begun { .. } => 1,
                                    _ => 0,
                                };
                                g.msgs.push_back(other);
                            }
                        }
                        drop(g);
                        reader.wake();
                    }
                    Ok(None) => break "the host ended the session".to_string(),
                    Err(e) => break e.to_string(),
                }
            };
            let mut g = reader.lock();
            if g.closed.is_none() {
                g.closed = Some(why);
            }
            drop(g);
            reader.wake();
        });
        let mut infos = HashMap::new();
        let mut order = Vec::new();
        for i in welcome.history {
            order.push(i.txn);
            infos.insert(i.txn, i);
        }
        Ok(Self {
            runtime: Some(runtime),
            endpoint,
            conn,
            out,
            shared,
            expected: Arc::new(AtomicU64::new(0)),
            predictor: Predictor::new(welcome.project, welcome.next_seq, welcome.issuer.clone()),
            issuer: welcome.issuer,
            session: welcome.session,
            device_id: welcome.device_id,
            infos,
            order,
            undo: welcome.undo,
            redo: welcome.redo,
            status: welcome.status.map(Arc::new),
            status_fresh: true,
            aliases: HashMap::new(),
            predicted: Vec::new(),
            refused_here: Vec::new(),
            next_ticket: 1,
            next_alias: 1,
            next_req: std::cell::Cell::new(1),
            closed_reported: false,
            opts,
        })
    }

    /// Wake the editor loop when answers arrive (from the connection's thread).
    pub fn set_waker(&mut self, w: Waker) {
        *self
            .shared
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(w);
    }

    /// A handle that outlives boxing this client into a shell.
    #[must_use]
    pub fn handle(&self) -> RemoteHandle {
        RemoteHandle {
            shared: Arc::clone(&self.shared),
            expected: Arc::clone(&self.expected),
            conn: self.conn.clone(),
        }
    }

    /// The host's id for this session (`remote-3`).
    #[must_use]
    pub fn session(&self) -> &str {
        &self.session
    }

    /// This device's id on the host (`dev-1`).
    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// The predicted project (the authoritative one plus pending predictions).
    #[must_use]
    pub fn predicted(&self) -> &Project {
        self.predictor.predicted()
    }

    /// Predictions not yet settled.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.predictor.pending()
    }

    fn ticket(&mut self) -> Ticket {
        let t = Ticket(self.next_ticket);
        self.next_ticket += 1;
        t
    }

    fn closed(&self) -> Option<String> {
        self.shared.lock().closed.clone()
    }

    /// Send a request the host will answer (counted for [`RemoteHandle::wait_settled`]); a
    /// closed session refuses it here, with the reason.
    fn request(&mut self, ticket: Ticket, what: impl FnOnce() -> String, r: Request) {
        if let Some(why) = self.closed() {
            self.refused_here.push(Refused {
                ticket,
                what: what(),
                rejection: Rejection::new(
                    CmdError::PolicyRefused {
                        what: format!("the remote core is not connected: {why}"),
                    },
                    None,
                    None,
                ),
            });
            return;
        }
        self.expected.fetch_add(1, Ordering::AcqRel);
        if self.out.send(r).is_err() {
            // The writer is gone: the session is closing; the reader reports why.
            self.expected.fetch_sub(1, Ordering::AcqRel);
        }
    }

    fn real(&self, t: TxnId) -> TxnId {
        if wire::is_alias(t) {
            self.aliases.get(&t.0).copied().unwrap_or(t)
        } else {
            t
        }
    }

    fn remember(&mut self, i: TxnInfo) {
        if !self.infos.contains_key(&i.txn) {
            self.order.push(i.txn);
        }
        self.infos.insert(i.txn, i);
    }

    fn reset_history(&mut self, h: Vec<TxnInfo>) {
        self.infos.clear();
        self.order.clear();
        for i in h {
            self.remember(i);
        }
    }
}

async fn open(
    cfg: quinn::ClientConfig,
    addr: SocketAddr,
    follow_session: bool,
) -> Result<
    (
        quinn::Endpoint,
        quinn::Connection,
        quinn::SendStream,
        quinn::RecvStream,
        HostMsg,
    ),
    RemoteError,
> {
    let (endpoint, conn) = crate::device::dial(cfg, addr).await?;
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| RemoteError::Transport(e.to_string()))?;
    wire::send(&mut send, &Hello::Session { follow_session }).await?;
    let first = wire::recv::<HostMsg>(&mut recv)
        .await?
        .ok_or_else(|| RemoteError::Refused("the host closed the session at once".into()))?;
    Ok((endpoint, conn, send, recv, first))
}

impl Drop for RemoteBus {
    fn drop(&mut self) {
        self.conn
            .close(0u32.into(), b"the client closed the session");
        if let Some(rt) = self.runtime.take() {
            // Let the close reach the host (bounded), so its session list and console show
            // the session ended now rather than at the idle timeout a minute later. Never
            // block inside an async context.
            if tokio::runtime::Handle::try_current().is_err() {
                let endpoint = self.endpoint.clone();
                rt.block_on(async move {
                    let _ = tokio::time::timeout(CLOSE_GRACE, endpoint.wait_idle()).await;
                });
            }
            rt.shutdown_background();
        }
    }
}

impl BusClient for RemoteBus {
    fn issuer(&self) -> &Issuer {
        &self.issuer
    }

    fn apply(&mut self, cmd: EditorCommand, txn: Option<TxnId>) -> Ticket {
        let t = self.ticket();
        if self.closed().is_none()
            && self.predictor.knows(&cmd)
            && let Some(diff) = self.predictor.predict(t.0, &cmd)
        {
            self.predicted.push(Prediction { ticket: t, diff });
        }
        let what = cmd.label();
        let cmd = self.opts.faults.mangle(cmd);
        self.request(
            t,
            || what,
            Request::Apply {
                ticket: t,
                cmd,
                txn,
            },
        );
        t
    }

    fn begin(&mut self, label: &str) -> TxnId {
        let alias = ALIAS_BIT | self.next_alias;
        self.next_alias += 1;
        if self.closed().is_none() {
            self.expected.fetch_add(1, Ordering::AcqRel);
            if self
                .out
                .send(Request::Begin {
                    alias,
                    label: label.to_string(),
                })
                .is_err()
            {
                self.expected.fetch_sub(1, Ordering::AcqRel);
            }
        }
        TxnId(alias)
    }

    fn commit(&mut self, txn: TxnId) -> Ticket {
        let t = self.ticket();
        self.request(
            t,
            || format!("Commit {txn}"),
            Request::Commit { ticket: t, txn },
        );
        t
    }

    fn cancel(&mut self, txn: TxnId) -> Ticket {
        let t = self.ticket();
        self.request(
            t,
            || format!("Cancel {txn}"),
            Request::Cancel { ticket: t, txn },
        );
        t
    }

    fn undo(&mut self, txn: TxnId) -> Ticket {
        let t = self.ticket();
        self.request(
            t,
            || format!("Undo {txn}"),
            Request::Undo { ticket: t, txn },
        );
        t
    }

    fn redo(&mut self, txn: TxnId) -> Ticket {
        let t = self.ticket();
        self.request(
            t,
            || format!("Redo {txn}"),
            Request::Redo { ticket: t, txn },
        );
        t
    }

    fn undo_target(&self) -> Option<TxnId> {
        self.undo
    }

    fn redo_target(&self) -> Option<TxnId> {
        self.redo
    }

    fn preview(&self, cmd: &EditorCommand) -> Result<Diff, Rejection> {
        if self.predictor.knows(cmd) {
            return self.predictor.preview(cmd);
        }
        // Only the core can plan it: ask, and wait (bounded).
        let refuse =
            |what: String| Err(Rejection::new(CmdError::PolicyRefused { what }, None, None));
        let req = self.next_req_id();
        if self
            .out
            .send(Request::Preview {
                req,
                cmd: cmd.clone(),
            })
            .is_err()
        {
            return refuse("the remote core is not connected".into());
        }
        let end = Instant::now() + self.opts.preview_timeout;
        let mut g = self.shared.lock();
        loop {
            if let Some(r) = g.previews.remove(&req) {
                return r;
            }
            if let Some(why) = &g.closed {
                return refuse(format!("the remote core is not connected: {why}"));
            }
            let now = Instant::now();
            if now >= end {
                return refuse(format!(
                    "the remote core did not answer the dry run of {} in time",
                    cmd.label()
                ));
            }
            g = self
                .shared
                .cv
                .wait_timeout(g, end - now)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
    }

    fn txn_info(&self, txn: TxnId) -> Option<TxnInfo> {
        self.infos.get(&self.real(txn)).cloned()
    }

    fn history(&self) -> Vec<TxnInfo> {
        // As the bus lists it: committed and undone transactions (a cancelled gesture or a
        // command that changed nothing never enters the history).
        self.order
            .iter()
            .filter_map(|t| self.infos.get(t))
            .filter(|i| matches!(i.state, TxnState::Committed | TxnState::Undone))
            .cloned()
            .collect()
    }

    fn snapshot(&self) -> (Project, u64) {
        (self.predictor.authoritative(), self.predictor.next_seq())
    }

    fn pump(&mut self) -> Pumped {
        let (msgs, closed) = {
            let mut g = self.shared.lock();
            (std::mem::take(&mut g.msgs), g.closed.clone())
        };
        let mut out = Pumped::default();
        let mut since_snapshot: Vec<Arc<Applied>> = Vec::new();
        let mut settled: Vec<Ticket> = Vec::new();
        let mut resync = false;
        for m in msgs {
            match m {
                HostMsg::Batch(b) => {
                    let b = *b;
                    if let Some(h) = b.history {
                        self.reset_history(h);
                    }
                    for i in b.infos {
                        self.remember(i);
                    }
                    let events = wire::shared(b.events);
                    since_snapshot.extend(events.iter().cloned());
                    out.events.extend(events);
                    settled.extend(b.settled);
                    out.refused.extend(b.refused);
                    out.session.extend(b.session);
                    out.session_dropped += b.session_dropped;
                    self.undo = b.undo;
                    self.redo = b.redo;
                    if let Some(s) = b.status {
                        let s = Arc::new(s);
                        self.status = Some(Arc::clone(&s));
                        out.project = Some(s);
                    }
                }
                HostMsg::Snapshot(s) => {
                    let s = *s;
                    // What came before the snapshot is in it; settle what is settled.
                    let ids: Vec<u64> = settled.iter().map(|t| t.0).collect();
                    let _ = self.predictor.reconcile(&since_snapshot, &ids);
                    since_snapshot.clear();
                    self.predictor.resync(s.project, s.next_seq);
                    self.reset_history(s.history);
                    self.undo = s.undo;
                    self.redo = s.redo;
                    out.gap = Some(s.gap.unwrap_or(Gap {
                        first_missed: s.next_seq,
                        last_missed: s.next_seq,
                        missed: 0,
                    }));
                }
                HostMsg::Begun { alias, txn } => {
                    self.aliases.insert(alias, txn);
                }
                HostMsg::Welcome(_) | HostMsg::Preview { .. } | HostMsg::Closed { .. } => {}
            }
        }
        let ids: Vec<u64> = settled.iter().map(|t| t.0).collect();
        if self.predictor.reconcile(&since_snapshot, &ids).is_err() {
            // The replica missed something: start again from a snapshot.
            resync = true;
        }
        if resync {
            let _ = self.out.send(Request::Resync);
        }
        if let Some(why) = closed
            && !self.closed_reported
        {
            self.closed_reported = true;
            out.refused.push(Refused {
                ticket: Ticket(0),
                what: "The remote session".into(),
                rejection: Rejection::new(
                    CmdError::PolicyRefused {
                        what: format!("the remote core ended the session: {why}"),
                    },
                    None,
                    None,
                ),
            });
        }
        out.refused.append(&mut self.refused_here);
        out.settled = settled;
        out.predicted = std::mem::take(&mut self.predicted);
        if self.status_fresh {
            self.status_fresh = false;
            if out.project.is_none() {
                out.project = self.status.clone();
            }
        }
        out
    }

    fn follow_session(&mut self, on: bool) {
        let _ = self.out.send(Request::FollowSession(on));
    }

    fn project_status(&self) -> Option<Arc<ProjectStatus>> {
        self.status.clone()
    }
}

impl RemoteBus {
    fn next_req_id(&self) -> u64 {
        let r = self.next_req.get();
        self.next_req.set(r + 1);
        r
    }
}

/// The server name a device dials (the verifier pins fingerprints, not names).
pub(crate) const SERVER_NAME: &str = HOST_NAME;
