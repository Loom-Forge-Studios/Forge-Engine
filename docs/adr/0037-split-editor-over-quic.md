# ADR 0037 — The split editor over QUIC: a remote BusClient, device pairing by key exchange, grants per device, and the optimistic overlay

- **Status:** accepted
- **Date:** 2026-09-23

## Context

M2-16 asks for the split editor v1: commands and state deltas over QUIC on a LAN between a
headless core and a UI client, local panels, a locally rendered viewport, the O-13
optimistic `dry_run` preview with reconciliation, device pairing and loopback by default,
guarded by `test_split_editor_parity` and `test_optimistic_reconciliation`. WP-U9 built the
Remote panel (pairing dialog, session list, latency) over the `RemoteTransport` trait with
the labelled in-memory `LoopbackTransport` . The shell already talks to the core only
through `BusClient`, which answers requests with tickets and outcomes through `pump` (ADR
0018), so the remote client can be written without touching a panel.

## Decision

1. **`crates/forge-remote`** sits above `forge-editor`. The **host** (`QuicTransport`) serves an
   `EditorCore` to paired devices: each session is a client of the core through the core's own
   `LocalBus`, so the core has no remote code path at all. The **client** (`RemoteBus`) is a
   `BusClient`; the shell, every panel and the viewport run on it unchanged (the viewport renders
   the mirror locally). `QuicTransport` implements WP-U9's `RemoteTransport`, and the editor
   binary hosts it on every GUI start (`--no-remote-host` turns it off); `--connect ADDR
   [--pair CODE]` runs the editor as a device. The loopback stand-in stays for tests that script
   "the other device".
2. **Wire.** QUIC (`quinn`, TLS 1.3, `ring`), one ordered bidirectional stream per session,
   `postcard` frames with a `u32` length (64 MiB cap). Payloads are the bus's own types —
   `EditorCommand` up, `Applied` down — so a delta is the reflect-tree diff the core applied. To
   carry them, `forge-cmd` gains serde on `Applied`, `AppliedKind`, `Rejection`, `CmdError`,
   `TxnState`, `Gap`, and a **canonical wire form for `Project`** whose deserializer rebuilds the
   project through the same checked `apply_all` every command goes through (names, paths and
   values validated, a missing parent or a cycle refused); `forge-project`'s status types and the
   editor's `TxnInfo` / `Refused` / `SessionCommand` / `Ticket` are serializable too (all
   additive). A host answers each batch of requests with **one** message: the events, the
   records of their transactions, the tickets it settled and any refusals — so a client never
   sees an answer before the events it answers.
3. **Nothing on the input path waits a round trip.** A transaction the client opens is named by
   an alias (`TxnId` with bit 62 set) until the host's `Begun` says its id; the host maps it.
   `snapshot`, `history`, `txn_info` and the undo targets are served from state the host streams.
   Only a dry run of a command the client cannot plan (a plugin command installed only on the
   host) asks the host and waits (2 s bound).
4. **O-13: the dry run *is* the prediction.** `forge_cmd::Replica` is a client's copy of the
   project that follows the `Applied` stream and carries pending predictions: `reconcile` rolls
   them back (inverses, newest first; a predicted spawn does not move the key allocator), applies
   the authoritative events, forgets settled ones and re-applies the rest. `Bus::plan_for` plans a
   command against another project with the bus's planners, policies and reserved settings;
   `forge_editor::core::Predictor` pairs the editor's planners with a replica (in `core`, the one
   module allowed to name the bus). `Pumped` gains `predicted` and `settled`; the **mirror** shows
   each prediction as an overlay the turn it is made and, in the pump its answer arrives in, lifts
   the overlays, applies the events and re-lays only the unsettled ones, each checked against the
   authoritative state first — the one-frame correction of network prediction.
5. **Pairing** needs a person on both devices and never sends the code. The device runs SPAKE2
   over the code its person typed; both sides prove the key with an HMAC over **both certificate
   fingerprints**, which binds the exchange to this TLS connection (a relay terminating TLS
   presents other fingerprints). A man in the middle gets one online guess per exchange, and the
   host withdraws a code after 3 failed exchanges. A verified request is listed for a person, who
   allows or denies it in the Remote panel; the pairing store records the device with its
   certificate fingerprint (`PairedDevice::fingerprint`, `pair_verified`), and the device pins the
   host's. Certificates are self-signed per machine (`rcgen`), kept in the user config.
6. **Grants per device, in the one table.** A paired device's capabilities live in its
   pairing record (user config: a paired device is a per-machine credential, so a grant for
   "dev-3" must not ship with the project to a teammate whose dev-3 is another machine). The
   host mirrors them into the core's `SharedGrants` as `Principal::Remote(<device id>)`
   through the store's observer and revokes them on unpairing, which also closes the
   device's sessions. A new pairing gets `Fs(ProjectRead)` + `Command(Ordinary)`; the
   destructive class is only added by a person (`RemotePairingStore::set_capabilities`,
   audited). A session commits or cancels only transactions it opened, and never sends the
   project load (`forge.project.load` replaces the security settings too; the core performs
   it when a project is opened, and its own non-human check cannot stop a remote session,
   whose issuer is a person). Its issuer is fixed by the host from the pairing
   (`human:<device-name>@<device-id>`). Sessions, denials and refused connections are
   audited.
7. **Loopback by default.** The host binds `127.0.0.1`; the pairing store's LAN exposure (the
   panel's confirmed, audited choice) rebinds the endpoint to every interface on the same port,
   through the observer. Internet exposure is not offered.
8. **The headless core is a host (amendment, verifier finding).** `forge --headless
   --remote-host` and `forge-editor --headless --remote-host` serve the core over the same
   `QuicTransport` with no window, GPU or panels (`forge_remote::console::serve`); `--port N`
   picks the port (default 47900, any free one if taken), `--lan` records the LAN exposure in
   the pairing config exactly as the panel's confirmed choice (typing the flag is the
   confirmation; without it the config's exposure stands, loopback unless a person chose the
   LAN). stdin becomes the **host console**: a person shows a code (`pair`), sees each request
   as it arrives and answers it (`allow NAME` / `deny NAME`), and sets a device's capabilities
   (`grant` / `revoke ID CAP`) — the panel's own calls on the transport and the allow-listed
   pairing store, audited under the OS user. The console has no path into project state; a
   remote session is the same peer of the core as with the GUI host. The console blocks on one
   channel fed by the terminal's reader thread and the transport's change feed, so an idle
   headless host makes no wakeups of its own; input ending (a service without a terminal) keeps
   the host serving the devices already paired. A dropped `RemoteBus` now waits up to 250 ms for
   its QUIC close to leave, so a host lists the session as ended at once instead of at the 60 s
   idle timeout.

## Why — the owner's two rules

1. **Better for the user**: the laptop runs the real editor — menus, inspector, typing, the
   viewport — locally and native; a drag shows the value the frame it happens even over a slow
   link (the reconciliation test runs a 300 ms link), and a wrong guess is gone one frame after
   the answer; pairing is a six-digit code and one click on each side, and it cannot be stolen by
   a machine in between; a paired laptop cannot delete or grant anything until a person says so.
2. **Faster / more efficient**: deltas are the bus's diffs in a binary codec on one stream (no
   per-message connection or JSON); nothing blocks the UI thread on the network; the host answers a
   burst of requests in one batch; an idle split editor sleeps (QUIC keep-alives run on the stack's
   own thread; the host's latency sample runs only while a session is open and reaches the panel
   through its ≤ 2 Hz live feed).

## Alternatives rejected

- **Blocking `begin` / `preview` round trips** — simple, but every gesture start and every hover
  preview would pay the link's latency: the split editor would feel worse than a streamer on the
  one interaction it exists to win.
- **Hashing or sending the pairing code** — a six-digit code is exhausted offline in well under a
  second; only a PAKE makes a short code safe against a machine in the middle.
- **A privileged remote path into the core** — the session is an ordinary `LocalBus` client,
  checked in the same grant table, under the same bus policies.
- **Predicting on the mirror itself** — the mirror has no planners; planning with the bus's own
  planners over a replica makes the prediction exactly the dry run (I8: `dry_run` equals
  `apply`'s diff), so a correct prediction never flickers.

## Consequences

- Gate rows (`Bound`): `C-split-editor-parity`, `C-optimistic-reconciliation`,
  `C-remote-transport-backend` (the Remote panel over real QUIC), `C-remote-grants`,
  `C-split-editor-idle`, `C-remote-exposure`, `C-split-editor-headless-host` (the `forge
  --headless --remote-host` process pairs at its console and passes the parity script). `Unbuilt`:
  `C-split-editor-viewport-stream` (thin-client video, Spike S11, M5), `C-split-editor-webrtc`
  (NAT traversal).
- Error codes `REMOTE-0001..0007`.
- Measured (dev box, release, loopback; `cargo run --release -p forge-remote --example
  split_measure`): pairing (TLS 1.3 handshake, SPAKE2, the allow) 5.6 ms;
  a session's welcome with a 10,000-entity project 24 ms (the canonical snapshot is 585 KiB,
  59 B per entity); a request's round trip (a property set answered with its event) median
  95 µs, p99 136 µs; a drag frame costs 106 B up and 191 B down on the wire, QUIC and TLS
  included. An idle connected editor draws 0 frames and wakes 0 times
  (`test_split_editor_idle`, gate `C-split-editor-idle`).
- Follow-ups: a device-side "Connect to a core" dialog (the CLI flags exist); the Remote panel's
  per-device capability control (the store API exists; granting destructive edits to a paired
  device is done through it today); the thin-client viewport stream (S11); WebRTC for NAT
  traversal.
