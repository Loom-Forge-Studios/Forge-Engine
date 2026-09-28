//! The device side: pair with a host, then open split-editor sessions on it.
//!
//! [`Device::pair`] runs the exchange of [`crate::pairing`] over a connection that pins
//! nothing yet, and succeeds only if (1) the host's key confirmation proves it used the same
//! code, over the certificate this device saw, and (2) a person at the host allowed it. The
//! [`PairedHost`] it returns pins the host's certificate for every later session
//! ([`crate::RemoteBus::connect`]); [`DeviceStore`] keeps it in the user config.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::RemoteError;
use crate::client::{ConnectOptions, RemoteBus, SERVER_NAME};
use crate::identity::{self, Identity};
use crate::pairing::{self, Exchange, Side};
use crate::wire::{self, DeviceConfirm, Hello, PairMsg};

/// Where a device keeps its identity, under the user config directory.
pub const DEVICE_IDENTITY_FILE: &str = "remote_device_identity.ron";
/// Where a device keeps the hosts it paired with.
pub const PAIRED_HOSTS_FILE: &str = "remote_hosts.ron";

/// A host this device paired with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedHost {
    pub addr: SocketAddr,
    /// The host certificate's fingerprint, pinned.
    pub fingerprint: String,
    /// This device's id on that host (`dev-1`).
    pub device_id: String,
}

/// This machine as a split-editor device.
#[derive(Clone, Debug)]
pub struct Device {
    identity: Identity,
    name: String,
}

pub(crate) async fn dial(
    cfg: quinn::ClientConfig,
    addr: SocketAddr,
) -> Result<(quinn::Endpoint, quinn::Connection), RemoteError> {
    let local: SocketAddr = if addr.is_ipv4() {
        (Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (Ipv6Addr::UNSPECIFIED, 0).into()
    };
    let mut endpoint =
        quinn::Endpoint::client(local).map_err(|e| RemoteError::Transport(e.to_string()))?;
    endpoint.set_default_client_config(cfg);
    let connecting = endpoint
        .connect(addr, SERVER_NAME)
        .map_err(|e| RemoteError::Transport(e.to_string()))?;
    let conn = connecting.await.map_err(|e| {
        let t = e.to_string();
        if t.contains("paired host") {
            RemoteError::Tls(t)
        } else {
            RemoteError::Transport(t)
        }
    })?;
    // The endpoint too: a client waits on it to let its close reach the host.
    Ok((endpoint, conn))
}

impl Device {
    /// This machine as a device named `name` (what the person at the host sees).
    #[must_use]
    pub fn new(identity: Identity, name: &str) -> Self {
        Self {
            identity,
            name: name.to_string(),
        }
    }

    /// Its identity (the certificate the host pins).
    #[must_use]
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Pair with the host at `addr` using the `code` its person showed; waits up to `wait`
    /// for a person there to allow it.
    pub fn pair(
        &self,
        addr: SocketAddr,
        code: &str,
        wait: Duration,
    ) -> Result<PairedHost, RemoteError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| RemoteError::Transport(e.to_string()))?;
        let r = rt.block_on(self.pair_async(addr, code, wait));
        rt.shutdown_background();
        r
    }

    async fn pair_async(
        &self,
        addr: SocketAddr,
        code: &str,
        wait: Duration,
    ) -> Result<PairedHost, RemoteError> {
        let (cfg, seen) = identity::client_config(&self.identity, None)?;
        let (_endpoint, conn) = dial(cfg, addr).await?;
        let host_fp = seen
            .lock()
            .ok()
            .and_then(|s| s.clone())
            .ok_or_else(|| RemoteError::Tls("the host presented no certificate".into()))?;
        let my_fp = self.identity.fingerprint().to_string();
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| RemoteError::Transport(e.to_string()))?;
        let (ex, msg) = Exchange::device(code);
        wire::send(
            &mut send,
            &Hello::Pair {
                name: self.name.clone(),
                spake: msg,
            },
        )
        .await?;
        let next = |r: Option<PairMsg>| {
            r.ok_or_else(|| RemoteError::Pairing("the host closed the pairing".into()))
        };
        let (spake, host_confirm) = match next(wire::recv::<PairMsg>(&mut recv).await?)? {
            PairMsg::Spake { spake, confirm } => (spake, confirm),
            PairMsg::Refused { why } | PairMsg::Denied { why } => {
                return Err(RemoteError::Pairing(why));
            }
            other => {
                return Err(RemoteError::Protocol(format!(
                    "expected the host's key exchange, got {other:?}"
                )));
            }
        };
        let key = ex.finish(&spake)?;
        let host_ok = pairing::check(&key, Side::Host, &host_fp, &my_fp, &host_confirm);
        // Confirm either way: the host lists (and audits) a failed attempt too.
        wire::send(
            &mut send,
            &DeviceConfirm {
                confirm: pairing::confirm(&key, Side::Device, &host_fp, &my_fp),
            },
        )
        .await?;
        if !host_ok {
            // Let the host take the confirmation (it lists and audits the attempt) before
            // the connection goes.
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(2), wire::recv::<PairMsg>(&mut recv))
                .await;
            conn.close(5u32.into(), b"key confirmation failed");
            return Err(RemoteError::Pairing(
                "the code is not the one on show on the host (or something between the two machines answered for it)".into(),
            ));
        }
        let answer = tokio::time::timeout(wait, async {
            loop {
                match next(wire::recv::<PairMsg>(&mut recv).await?)? {
                    PairMsg::Waiting => {}
                    m => return Ok::<PairMsg, RemoteError>(m),
                }
            }
        })
        .await
        .map_err(|_| {
            RemoteError::Timeout("no one answered the pairing request on the host".into())
        })??;
        let _ = send.finish();
        conn.close(0u32.into(), b"paired");
        match answer {
            PairMsg::Paired { device_id } => Ok(PairedHost {
                addr,
                fingerprint: host_fp,
                device_id,
            }),
            PairMsg::Denied { why } | PairMsg::Refused { why } => Err(RemoteError::Pairing(why)),
            other => Err(RemoteError::Protocol(format!(
                "expected the host's answer, got {other:?}"
            ))),
        }
    }

    /// Open a split-editor session on `host`.
    pub fn connect(
        &self,
        host: &PairedHost,
        opts: ConnectOptions,
    ) -> Result<RemoteBus, RemoteError> {
        RemoteBus::connect(&self.identity, host, opts)
    }
}

/// The hosts a device paired with (user config).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStore {
    pub hosts: Vec<PairedHost>,
    #[serde(skip)]
    dir: Option<PathBuf>,
}

impl DeviceStore {
    /// The store under `dir` (`None`: memory only).
    pub fn open(dir: Option<&Path>) -> Result<Self, RemoteError> {
        let mut s = match dir {
            Some(d) => {
                let path = d.join(PAIRED_HOSTS_FILE);
                match forge_editor::user_config::read_optional(&path)
                    .map_err(|e| RemoteError::Config(e.to_string()))?
                {
                    Some(t) => ron::from_str(&t)
                        .map_err(|e| RemoteError::Config(format!("{}: {e}", path.display())))?,
                    None => Self::default(),
                }
            }
            None => Self::default(),
        };
        s.dir = dir.map(Path::to_path_buf);
        Ok(s)
    }

    /// Remember `h` (replacing an earlier pairing with the same address).
    pub fn remember(&mut self, h: PairedHost) -> Result<(), RemoteError> {
        self.hosts.retain(|x| x.addr != h.addr);
        self.hosts.push(h);
        let Some(d) = &self.dir else { return Ok(()) };
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| RemoteError::Config(e.to_string()))?;
        forge_editor::user_config::write_atomic(&d.join(PAIRED_HOSTS_FILE), text.as_bytes())
            .map_err(|e| RemoteError::Config(e.to_string()))
    }

    /// The pairing for `addr`.
    #[must_use]
    pub fn get(&self, addr: SocketAddr) -> Option<&PairedHost> {
        self.hosts.iter().find(|h| h.addr == addr)
    }
}

/// Wait for `f` (real time, polling every few milliseconds; tests and tools).
pub fn wait_for(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = std::time::Instant::now() + timeout;
    loop {
        if f() {
            return true;
        }
        if std::time::Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
