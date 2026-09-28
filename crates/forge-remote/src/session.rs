//! One remote editor session on the host: a paired device's connection bridged to the core
//! through the core's own `LocalBus` (see [`crate::host`]).
//!
//! **A peer, never a privileged path.** Every request is checked in the core's one grant
//! table as `Principal::Remote(<device id>)` — with the capability class every non-human
//! principal needs (`forge_plugin::CommandClass::of`: a destructive or security-class command needs
//! `Command(Destructive)`; undoing someone else's work too) — and only then handed to the bus,
//! whose policies (human-only commands, reserved settings, transaction ownership) then apply
//! exactly as to a local client. A session may commit or cancel only transactions it opened.
//! Denials are refusals the client sees, and audited.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use forge_cmd::{CmdError, Issuer, Rejection, TxnId};
use forge_editor::client::{BusClient, Refused, Ticket, TxnInfo};
use forge_editor::connect::audit::AuditOrigin;
use forge_editor::connect::remote::{PairedDevice, RemoteSession};
use forge_editor::core::{EditorCore, LocalBus};
use forge_plugin::{Capability, CommandClass, FsScope, Principal};
use tokio::sync::{Notify, mpsc};

use crate::RemoteError;
use crate::host::{Live, Shared};
use crate::wire::{self, Batch, HostMsg, Request, Snapshot, Welcome, is_alias};

/// The issuer a paired device's requests carry: the device's name and id, fixed by the host
/// (`studio-laptop@dev-1`, tagged `human:studio-laptop@dev-1`): a person at a device a
/// person here paired.
fn issuer_of(d: &PairedDevice) -> Issuer {
    let name: String = d
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    Issuer::Human {
        user: format!("{name}@{}", d.id),
    }
}

struct Session {
    shared: Arc<Shared>,
    local: LocalBus,
    device: PairedDevice,
    principal: Principal,
    id: String,
    /// Client aliases of the transactions this session opened.
    aliases: HashMap<u64, TxnId>,
    /// Transactions this session opened (it may commit or cancel only these).
    own: BTreeSet<TxnId>,
    /// The core client's tickets of this flush's requests, to the remote tickets.
    tickets: BTreeMap<Ticket, Ticket>,
    settled: Vec<Ticket>,
    refused: Vec<Refused>,
    seen_epoch: Option<u64>,
    out: mpsc::UnboundedSender<(tokio::time::Instant, HostMsg)>,
}

pub(crate) async fn run(
    shared: Arc<Shared>,
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    device: PairedDevice,
    follow_session: bool,
) -> Result<(), RemoteError> {
    let principal = Principal::Remote(device.id.clone());
    if !shared.faults.skip_grant_check()
        && !shared
            .grants
            .has(&principal, Capability::Fs(FsScope::ProjectRead))
    {
        let _ = wire::send(
            &mut send,
            &HostMsg::Closed {
                why: format!(
                    "{} may not read the project ({} is not granted to it here)",
                    device.id,
                    Capability::Fs(FsScope::ProjectRead)
                ),
            },
        )
        .await;
        let _ = send.finish();
        return Ok(());
    }
    let issuer = issuer_of(&device);
    let mut local = EditorCore::connect(&shared.core, issuer.clone());
    local.follow_session(follow_session);
    let notify = Arc::new(Notify::new());
    let n = Arc::clone(&notify);
    local.set_waker(Arc::new(move || n.notify_one()));

    let id = {
        let mut st = shared.lock();
        let id = format!("remote-{}", st.next_session);
        st.next_session += 1;
        st.sessions.push(Live {
            info: RemoteSession {
                id: id.clone(),
                device: device.name.clone(),
                started_at_ms: shared.audit.now_ms(),
                commands: 0,
            },
            device_id: device.id.clone(),
            conn: conn.clone(),
        });
        shared.touched(&mut st);
        id
    };
    shared.latency.set_live(true);
    shared.latency.record(conn.rtt().as_secs_f64() * 1000.0);
    shared.audit.record(
        AuditOrigin::Remote,
        &id,
        &issuer.tag(),
        "connected",
        format!("{id} from {} ({})", device.name, device.id),
    );

    // Everything the host sends goes through one writer task, in order.
    let (out, mut outbox) = mpsc::unbounded_channel::<(tokio::time::Instant, HostMsg)>();
    let delay = shared.delay;
    let writer = tokio::spawn(async move {
        while let Some((at, m)) = outbox.recv().await {
            if !delay.is_zero() {
                tokio::time::sleep_until(at + delay).await;
            }
            if wire::send(&mut send, &m).await.is_err() {
                break;
            }
        }
        let _ = send.finish();
    });
    // Requests arrive through one reader task (a frame read is not cancel-safe).
    let (req_tx, mut requests) = mpsc::unbounded_channel::<Request>();
    let reader = tokio::spawn(async move {
        while let Ok(Some(r)) = wire::recv::<Request>(&mut recv).await {
            if req_tx.send(r).is_err() {
                break;
            }
        }
    });

    let mut s = Session {
        shared: Arc::clone(&shared),
        local,
        principal,
        id: id.clone(),
        device,
        aliases: HashMap::new(),
        own: BTreeSet::new(),
        tickets: BTreeMap::new(),
        settled: Vec::new(),
        refused: Vec::new(),
        seen_epoch: None,
        out,
    };
    s.welcome();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            r = requests.recv() => match r {
                Some(req) => {
                    s.handle(req);
                    // Take whatever else already arrived before answering (one batch).
                    while let Ok(more) = requests.try_recv() {
                        s.handle(more);
                    }
                    s.flush();
                }
                None => break,
            },
            () = notify.notified() => s.flush(),
            _ = tick.tick() => {
                // The link's latency, for the host's Remote panel (a live feed it shows at
                // most twice a second, only while visible). Nothing is sent to the client.
                shared.latency.record(conn.rtt().as_secs_f64() * 1000.0);
                if shared.faults.heartbeat() {
                    s.send(HostMsg::Batch(Box::default()));
                }
            }
            _ = conn.closed() => break,
        }
    }
    reader.abort();
    drop(s.out);
    let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
    {
        let mut st = shared.lock();
        st.sessions.retain(|l| l.info.id != id);
        if st.sessions.is_empty() {
            shared.latency.set_live(false);
        }
        shared.touched(&mut st);
    }
    shared.audit.record(
        AuditOrigin::Remote,
        &id,
        &issuer.tag(),
        "disconnected",
        format!("{id} ended"),
    );
    conn.close(0u32.into(), b"session ended");
    Ok(())
}

impl Session {
    fn send(&self, m: HostMsg) {
        let _ = self.out.send((tokio::time::Instant::now(), m));
    }

    fn welcome(&mut self) {
        let (project, next_seq) = self.local.snapshot();
        let status = self.local.project_status().map(|s| (*s).clone());
        self.seen_epoch = status.as_ref().map(|s| s.epoch);
        self.send(HostMsg::Welcome(Box::new(Welcome {
            session: self.id.clone(),
            device_id: self.device.id.clone(),
            issuer: self.local.issuer().clone(),
            project,
            next_seq,
            history: self.local.history(),
            undo: self.local.undo_target(),
            redo: self.local.redo_target(),
            status,
        })));
    }

    fn snapshot(&self, gap: Option<forge_cmd::Gap>) -> HostMsg {
        let (project, next_seq) = self.local.snapshot();
        HostMsg::Snapshot(Box::new(Snapshot {
            gap,
            project,
            next_seq,
            history: self.local.history(),
            undo: self.local.undo_target(),
            redo: self.local.redo_target(),
        }))
    }

    fn counted(&self) {
        let mut st = self.shared.lock();
        if let Some(l) = st.sessions.iter_mut().find(|l| l.info.id == self.id) {
            l.info.commands += 1;
        }
    }

    fn refuse(&mut self, ticket: Ticket, what: String, error: CmdError) {
        self.shared.audit.record(
            AuditOrigin::Remote,
            &self.id,
            &self.local.issuer().tag(),
            "denied",
            format!("{what}: {error}"),
        );
        self.refused.push(Refused {
            ticket,
            what,
            rejection: Rejection::new(error, None, None),
        });
        self.settled.push(ticket);
    }

    /// `Ok` if this device holds `cap` now, else the refusal.
    fn allowed(&self, cap: Capability) -> Result<(), CmdError> {
        if self.shared.faults.skip_grant_check() || self.shared.grants.has(&self.principal, cap) {
            Ok(())
        } else {
            Err(CmdError::PolicyRefused {
                what: format!(
                    "the remote device {} (\u{201c}{}\u{201d}) needs {cap}; a person grants it on the host",
                    self.device.id, self.device.name
                ),
            })
        }
    }

    fn resolve(&self, t: TxnId) -> Result<TxnId, CmdError> {
        if is_alias(t) {
            self.aliases
                .get(&t.0)
                .copied()
                .ok_or(CmdError::UnknownTxn(t))
        } else {
            Ok(t)
        }
    }

    fn sent(&mut self, local: Ticket, remote: Ticket) {
        self.tickets.insert(local, remote);
        self.settled.push(remote);
    }

    fn handle(&mut self, req: Request) {
        self.counted();
        match req {
            Request::Apply { ticket, cmd, txn } => {
                let what = cmd.label();
                let txn = match txn.map(|t| self.resolve(t)).transpose() {
                    Ok(t) => t,
                    Err(e) => return self.refuse(ticket, what, e),
                };
                // The project load replaces the whole project, its security settings included:
                // the core performs it when a project is opened, created or pulled. A remote
                // session's issuer is a person, so the core's own non-human check does not
                // stop it: refused here, whatever the device is granted.
                if matches!(&cmd, forge_cmd::EditorCommand::Invoke { target, .. } if target == forge_project::format::LOAD_CMD)
                    && !self.shared.faults.skip_grant_check()
                {
                    return self.refuse(
                        ticket,
                        what,
                        CmdError::PolicyRefused {
                            what: format!(
                                "{} is performed by the core when a project is opened, created or pulled; a remote session may not send it",
                                forge_project::format::LOAD_CMD
                            ),
                        },
                    );
                }
                let class =
                    forge_plugin::CommandClass::of(&cmd, self.local.policy_of(&cmd).human_only);
                if let Err(e) = self.allowed(Capability::Command(class)) {
                    return self.refuse(ticket, what, e);
                }
                let lt = self.local.apply(cmd, txn);
                self.sent(lt, ticket);
            }
            Request::Begin { alias, label } => {
                let real = self.local.begin(&label);
                self.aliases.insert(alias, real);
                self.own.insert(real);
                self.send(HostMsg::Begun { alias, txn: real });
            }
            Request::Commit { ticket, txn } => self.close_txn(ticket, txn, false),
            Request::Cancel { ticket, txn } => self.close_txn(ticket, txn, true),
            Request::Undo { ticket, txn } => self.history_step(ticket, txn, false),
            Request::Redo { ticket, txn } => self.history_step(ticket, txn, true),
            Request::Preview { req, cmd } => {
                let result = self
                    .allowed(Capability::Fs(FsScope::ProjectRead))
                    .map_err(|e| Rejection::new(e, None, None))
                    .and_then(|()| self.local.preview(&cmd));
                self.send(HostMsg::Preview { req, result });
            }
            Request::FollowSession(on) => self.local.follow_session(on),
            Request::Resync => {
                let m = self.snapshot(None);
                self.send(m);
            }
        }
    }

    fn close_txn(&mut self, ticket: Ticket, txn: TxnId, cancel: bool) {
        let what = if cancel { "Cancel" } else { "Commit" };
        let real = match self.resolve(txn) {
            Ok(t) => t,
            Err(e) => return self.refuse(ticket, format!("{what} {txn}"), e),
        };
        if !self.own.contains(&real) {
            return self.refuse(
                ticket,
                format!("{what} {real}"),
                CmdError::PolicyRefused {
                    what: format!("{real} was not opened by this remote session"),
                },
            );
        }
        let lt = if cancel {
            self.local.cancel(real)
        } else {
            self.local.commit(real)
        };
        self.sent(lt, ticket);
    }

    fn history_step(&mut self, ticket: Ticket, txn: TxnId, redo: bool) {
        let what = if redo { "Redo" } else { "Undo" };
        let real = match self.resolve(txn) {
            Ok(t) => t,
            Err(e) => return self.refuse(ticket, format!("{what} {txn}"), e),
        };
        // Undoing someone else's work destroys it: the destructive class (as for an automation
        // session).
        let own = self
            .local
            .txn_info(real)
            .is_some_and(|i: TxnInfo| &i.issuer == self.local.issuer());
        let class = if own {
            CommandClass::Ordinary
        } else {
            CommandClass::Destructive
        };
        if let Err(e) = self.allowed(Capability::Command(class)) {
            return self.refuse(ticket, format!("{what} {real}"), e);
        }
        let lt = if redo {
            self.local.redo(real)
        } else {
            self.local.undo(real)
        };
        self.sent(lt, ticket);
    }

    /// Send what happened since the last flush, in one batch.
    fn flush(&mut self) {
        let p = self.local.pump();
        if p.gap.is_some() {
            let m = self.snapshot(p.gap);
            self.send(m);
        }
        let mut refused = std::mem::take(&mut self.refused);
        for r in p.refused {
            let ticket = self.tickets.get(&r.ticket).copied().unwrap_or(Ticket(0));
            refused.push(Refused { ticket, ..r });
        }
        self.tickets.clear();
        let mut infos: Vec<TxnInfo> = Vec::new();
        let mut seen = BTreeSet::new();
        for e in &p.events {
            if seen.insert(e.txn)
                && let Some(i) = self.local.txn_info(e.txn)
            {
                infos.push(i);
            }
        }
        let status = p.project.map(|s| (*s).clone());
        let history = match &status {
            Some(st) if Some(st.epoch) != self.seen_epoch => {
                self.seen_epoch = Some(st.epoch);
                Some(self.local.history())
            }
            _ => None,
        };
        let batch = Batch {
            events: p.events.iter().map(|a| (**a).clone()).collect(),
            infos,
            settled: std::mem::take(&mut self.settled),
            refused,
            session: p.session,
            session_dropped: p.session_dropped,
            undo: self.local.undo_target(),
            redo: self.local.redo_target(),
            status,
            history,
        };
        let empty = batch.events.is_empty()
            && batch.settled.is_empty()
            && batch.refused.is_empty()
            && batch.session.is_empty()
            && batch.session_dropped == 0
            && batch.status.is_none();
        if !empty {
            self.send(HostMsg::Batch(Box::new(batch)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_cmd::EditorCommand;

    #[test]
    fn a_device_issues_as_itself() {
        let d = PairedDevice {
            id: "dev-3".into(),
            name: "Ada's laptop".into(),
            allowed_by: "ada".into(),
            paired_at_ms: 0,
            fingerprint: String::new(),
            capabilities: Vec::new(),
        };
        assert_eq!(issuer_of(&d).tag(), "human:Ada-s-laptop@dev-3");
    }

    #[test]
    fn an_unknown_command_is_ordinary() {
        let cmd = EditorCommand::Invoke {
            target: "x.y".into(),
            args: "{}".into(),
        };
        assert_eq!(
            forge_plugin::CommandClass::of(&cmd, false),
            CommandClass::Ordinary
        );
        assert_eq!(
            forge_plugin::CommandClass::of(&cmd, true),
            CommandClass::Destructive
        );
    }
}
