//! Shared set-up for the split-editor tests: a host serving a core over real QUIC on
//! loopback, its pairing store attached, and a device paired with it the way a person does
//! it (a code shown on the host, typed on the device, allowed on the host).
#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use forge_editor::connect::remote::{RemotePairingStore, RemoteTransport};
use forge_editor::core::{EditorCore, SharedCore};
use forge_remote::device::wait_for;
use forge_remote::{Device, HostConfig, HostFaults, Identity, PairedHost, QuicTransport};

/// A host over `core`, listening on an ephemeral loopback port.
pub struct Host {
    pub core: SharedCore,
    pub transport: QuicTransport,
    pub store: RemotePairingStore,
}

impl Host {
    pub fn new(core: SharedCore) -> Self {
        Self::with_faults(core, HostFaults::default())
    }

    pub fn with_faults(core: SharedCore, faults: HostFaults) -> Self {
        Self::with(core, faults, Duration::ZERO)
    }

    /// A host whose every message reaches the device `delay` late (a slow link).
    pub fn with(core: SharedCore, faults: HostFaults, delay: Duration) -> Self {
        let transport = QuicTransport::start(
            core.clone(),
            HostConfig {
                port: 0,
                faults,
                simulated_delay: delay,
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
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.transport
            .local_addr()
            .unwrap_or_else(|| panic!("the host listens"))
    }

    /// Pair a new device named `name`: show a code, the device types it, a person here
    /// allows it. Returns the device and its pinned host.
    pub fn pair(&mut self, name: &str) -> (Device, PairedHost) {
        let device = Device::new(
            Identity::generate("forge-device").unwrap_or_else(|e| panic!("{e}")),
            name,
        );
        let now = EditorCore::audit_book(&self.core).now_ms();
        let code = self.transport.begin_pairing(now).code;
        let addr = self.addr();
        let d = device.clone();
        let waiting = std::thread::spawn(move || d.pair(addr, &code, Duration::from_secs(30)));
        assert!(
            wait_for(Duration::from_secs(10), || !self
                .transport
                .requests()
                .is_empty()),
            "the device's request never arrived"
        );
        let answered = self
            .transport
            .answer(name, now)
            .unwrap_or_else(|e| panic!("{e}"));
        let fp = self.transport.take_verified(&answered);
        assert!(
            fp.is_some(),
            "a verified request carries the device's certificate"
        );
        self.store
            .pair_verified(&answered, fp.as_deref(), "tester", now)
            .unwrap_or_else(|e| panic!("{e}"));
        let host = waiting
            .join()
            .unwrap_or_else(|_| panic!("the device thread panicked"))
            .unwrap_or_else(|e| panic!("{e}"));
        (device, host)
    }

    /// Let device `id`'s sessions do everything a person can grant (destructive edits too).
    pub fn grant_all(&mut self, id: &str) {
        let caps = forge_plugin::Capability::ALL;
        self.store
            .set_capabilities(id, &caps, "tester")
            .unwrap_or_else(|e| panic!("{e}"));
    }
}

/// The editor shell's configuration with the stand-in panels (the core panels are not what
/// these tests are about).
pub fn shell_config() -> forge_editor::shell::ShellConfig {
    let stand_in = forge_editor::stand_in::StandInPanels::new().unwrap_or_else(|e| panic!("{e}"));
    let preset = forge_editor::presets::builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    forge_editor::shell::assemble(preset, &[&stand_in], &[], None).unwrap_or_else(|e| panic!("{e}"))
}

/// A headless editor whose shell is a **remote client** of `host`'s core: a device paired as
/// `name` connects over QUIC, and the shell runs on its `RemoteBus` (the split editor's
/// laptop side, with the workstation's core in this process only because the test hosts it).
pub fn remote_rig(
    host: &mut Host,
    cfg: forge_editor::shell::ShellConfig,
    name: &str,
) -> (
    forge_editor::testing::Rig,
    forge_remote::RemoteHandle,
    PairedHost,
) {
    let (device, paired) = host.pair(name);
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let put = std::rc::Rc::clone(&slot);
    let p = paired.clone();
    let rig = forge_editor::testing::Rig::with_client(cfg, host.core.clone(), move |(waker, _)| {
        let mut bus = device
            .connect(
                &p,
                forge_remote::ConnectOptions {
                    follow_session: true,
                    ..forge_remote::ConnectOptions::default()
                },
            )
            .unwrap_or_else(|e| panic!("{e}"));
        bus.set_waker(waker);
        *put.borrow_mut() = Some(bus.handle());
        Box::new(bus)
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let handle = slot
        .borrow_mut()
        .take()
        .unwrap_or_else(|| panic!("the client connected"));
    (rig, handle, paired)
}

/// Wait for every answer the host owes the rig's client, then run exactly one loop turn.
pub fn one_frame_after_answers(
    rig: &mut forge_editor::testing::Rig,
    handle: &forge_remote::RemoteHandle,
) {
    assert!(
        handle.wait_settled(Duration::from_secs(10)),
        "the host did not answer: {:?}",
        handle.closed()
    );
    rig.turn();
}
pub mod parity_script;
