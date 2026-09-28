//! **Remote connect** (`forge.remote`, Ch.21 §21.21, Ch.34; DoD M2-55): the split editor's
//! connect / pair dialog.
//!
//! * **Pair a device, with a person on both ends.** "Show pairing code" puts a one-time code
//!   on screen; the person at the other device types it there; its request appears here and
//!   a person here allows or denies it. A wrong or expired code is refused and audited.
//! * Pairing is **user config**, written by the allow-listed `RemotePairingStore` (never
//!   project state: a paired device is a per-machine credential), and audited.
//! * **Loopback by default**; exposing the host on the LAN asks first, and says what it
//!   means. Internet exposure is not offered (it needs a written acknowledgement in the
//!   config, Ch.34 §34.5).
//! * Remote sessions, and the link's **latency** as a live readout refreshed at most twice a
//!   second, only while the panel is visible.

use std::sync::Arc;

use forge_editor::connect::remote::Exposure;
use forge_editor::panels::PanelCx;
use forge_ui::widgets::{LabelKind, LiveReadout, Pressed};
use forge_ui::{LiveFeed, NodeStyle, Role, Ui, WidgetId};

use crate::relay::{FeedRelay, FeedTicked};
use crate::ui::{bar, button, fill, group, list, selected, set, show, text, warn};

fn by(act: &forge_editor::panel_rt::PanelAct) -> String {
    forge_editor::connect::audit::AuditRecord::user_of(&act.cmd.issuer())
}

fn nth(key: u64) -> Option<usize> {
    (key as usize).checked_sub(1)
}

struct Parts {
    requests: WidgetId,
    devices: WidgetId,
    sessions: WidgetId,
    /// The sessions list's empty state (M2-70).
    sessions_empty: WidgetId,
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let services = pb.services();
        let space = pb.b.theme_ref().space;
        let root = pb.b.add(
            pb.parent,
            "content",
            NodeStyle::column(space[1])
                .padding(space[2])
                .grow(1.0)
                .scrollable(),
            forge_ui::widgets::Container::new(Role::Group).labelled(forge_ui::tr!("Remote")),
        )?;
        let relay = pb.b.add(root, "feed", NodeStyle::leaf(), FeedRelay::new())?;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Heading)?;

        // ---- exposure ----
        let eg = group(pb, root, "exposure_group", forge_ui::tr!("Exposure"))?;
        let exposure = pb.b.signal(String::new());
        text(pb, eg, "exposure", exposure, LabelKind::Body)?;
        let ebar = bar(pb, eg, "exposure_bar", forge_ui::tr!("Exposure"))?;
        let to_lan = button(pb, ebar, "expose_lan", forge_ui::tr!("Expose on the LAN\u{2026}"))?;
        let to_loop = button(pb, ebar, "loopback", forge_ui::tr!("Back to loopback only"))?;
        let confirm = group(pb, eg, "lan_confirm", forge_ui::tr!("Expose on the LAN?"))?;
        text(
            pb,
            confirm,
            "lan_text",
            forge_ui::tr!("Exposing the editor on the local network lets every paired device on the network connect to this core. Unpaired devices still cannot. Expose it?"),
            LabelKind::Body,
        )?;
        let cbar = bar(pb, confirm, "lan_bar", forge_ui::tr!("Confirm"))?;
        let lan_yes = pb.b.add(
            cbar,
            "lan_yes",
            NodeStyle::leaf(),
            forge_ui::widgets::Button::new(forge_ui::tr!("Expose on the LAN")).primary(),
        )?;
        let lan_no = button(pb, cbar, "lan_no", forge_ui::tr!("Cancel"))?;
        pb.b.hide(confirm, true);
        text(
            pb,
            eg,
            "internet",
            forge_ui::tr!("Internet exposure is not offered here: it needs a written acknowledgement in the config (Ch.34 \u{a7}34.5)."),
            LabelKind::Small,
        )?;

        // ---- pairing ----
        let pg = group(pb, root, "pairing_group", forge_ui::tr!("Pair a device"))?;
        let code = pb.b.signal(String::new());
        text(pb, pg, "code", code, LabelKind::Heading)?;
        let pbar = bar(pb, pg, "pairing_bar", forge_ui::tr!("Pairing"))?;
        let show_code = pb.b.add(
            pbar,
            "show_code",
            NodeStyle::leaf(),
            forge_ui::widgets::Button::new(forge_ui::tr!("Show pairing code")).primary(),
        )?;
        let requests = list(pb, pg, "requests", forge_ui::tr!("Devices asking to pair"), 70.0)?;
        let rbar = bar(pb, pg, "requests_bar", forge_ui::tr!("Answer"))?;
        let allow = button(pb, rbar, "allow", forge_ui::tr!("Allow"))?;
        let deny = button(pb, rbar, "deny", forge_ui::tr!("Deny"))?;

        // ---- devices and sessions ----
        let dg = group(pb, root, "devices_group", forge_ui::tr!("Paired devices"))?;
        let devices = list(pb, dg, "devices", forge_ui::tr!("Paired devices"), 80.0)?;
        let unpair = button(pb, dg, "unpair", forge_ui::tr!("Unpair"))?;
        let sg = group(pb, root, "sessions_group", forge_ui::tr!("Remote sessions"))?;
        let sessions_empty = crate::ui::empty(
            pb,
            sg,
            "sessions_empty",
            forge_ui::tr!("No device is connected. Pair one below to edit this project from it."),
        )?;
        let sessions = list(pb, sg, "sessions", forge_ui::tr!("Remote sessions"), 80.0)?;
        let disconnect = button(pb, sg, "disconnect", forge_ui::tr!("Disconnect"))?;
        let latency_text = pb.b.signal("\u{2014}".to_string());
        let feed = services.connect.remote.borrow().latency();
        let read = Arc::clone(&feed);
        let readout = pb.b.add(
            sg,
            "latency",
            NodeStyle::leaf(),
            LiveReadout::new(forge_ui::tr!("Latency"), latency_text, move || match read.ms() {
                Some(ms) => format!("{ms:.0} ms"),
                None => forge_ui::tr!("\u{2014} (not measured)").into(),
            }),
        )?;
        let status = pb.b.signal(String::new());
        text(pb, root, "status", status, LabelKind::Muted)?;
        text(
            pb,
            root,
            "footer",
            forge_ui::trf!("Transport: {backend} \u{b7} pairing is kept in your user config{error}", backend = services.connect.remote.borrow().backend(), error = services
                    .connect
                    .pairing_error
                    .as_ref()
                    .map(|e| forge_ui::trf!(" (the file could not be read and was reset: {e})", e))
                    .unwrap_or_default()),
            LabelKind::Small,
        )?;

        let parts = std::rc::Rc::new(Parts {
            requests,
            devices,
            sessions,
            sessions_empty,
        });

        pb.on(to_lan, move |act, _: &Pressed| show(act.ui, confirm, true));
        pb.on(lan_no, move |act, _: &Pressed| show(act.ui, confirm, false));
        pb.on(lan_yes, move |act, _: &Pressed| {
            let who = by(act);
            let r = act
                .services
                .connect
                .pairing
                .borrow_mut()
                .set_exposure(Exposure::Lan, true, &who);
            show(act.ui, confirm, false);
            if let Err(e) = r {
                warn(act.session, forge_ui::tr!("The exposure was not changed"), &e);
            }
            act.want_turn();
        });
        pb.on(to_loop, move |act, _: &Pressed| {
            let who = by(act);
            let r = act
                .services
                .connect
                .pairing
                .borrow_mut()
                .set_exposure(Exposure::Loopback, false, &who);
            if let Err(e) = r {
                warn(act.session, forge_ui::tr!("The exposure was not changed"), &e);
            }
            act.want_turn();
        });
        pb.on(show_code, move |act, _: &Pressed| {
            let now = act.services.connect.book.now_ms();
            let o = act.services.connect.remote.borrow_mut().begin_pairing(now);
            set(
                act.ui,
                code,
                forge_ui::trf!("Pairing code: {code} \u{2014} type it on the other device within {seconds} s", code = o.code, seconds = forge_editor::connect::remote::PAIRING_CODE_MS / 1000),
            );
            act.want_turn();
        });
        for (btn, ok) in [(allow, true), (deny, false)] {
            let p = parts.clone();
            pb.on(btn, move |act, _: &Pressed| {
                let c = &act.services.connect;
                let req = selected(act.ui, p.requests)
                    .and_then(nth)
                    .and_then(|i| c.remote.borrow().requests().get(i).cloned());
                let Some(req) = req else {
                    set(act.ui, status, forge_ui::tr!("Select a request first.").into());
                    return;
                };
                let who = by(act);
                let now = c.book.now_ms();
                if !ok {
                    // Denied by the person here: the device is told so, the request is
                    // dropped, the code stays on show.
                    let why = forge_ui::trf!("{who} denied the pairing request on the host", who);
                    let _ = c.remote.borrow_mut().deny(&req.device, &why);
                    c.pairing.borrow().denied(&req.device, &who, forge_ui::tr!("denied here"));
                    set(act.ui, status, forge_ui::trf!("Denied \u{201c}{device}\u{201d}.", device = req.device));
                    act.want_turn();
                    return;
                }
                let answer = c.remote.borrow_mut().answer(&req.device, now);
                // The device's certificate, as the transport verified it (M2-16).
                let verified = answer
                    .as_ref()
                    .ok()
                    .and_then(|d| c.remote.borrow_mut().take_verified(d));
                match answer {
                    Ok(device) => match c.pairing.borrow_mut().pair_verified(
                        &device,
                        verified.as_deref(),
                        &who,
                        now,
                    ) {
                        Ok(d) => {
                            set(act.ui, code, String::new());
                            set(
                                act.ui,
                                status,
                                forge_ui::trf!("Paired \u{201c}{name}\u{201d} as {id}.", name = d.name, id = d.id),
                            );
                        }
                        Err(e) => warn(act.session, forge_ui::tr!("The device was not paired"), &e),
                    },
                    Err(e) => {
                        c.pairing.borrow().denied(&req.device, &who, &e.to_string());
                        set(act.ui, status, e.to_string());
                        warn(act.session, forge_ui::tr!("The device was not paired"), &e);
                    }
                }
                act.want_turn();
            });
        }
        {
            let p = parts.clone();
            pb.on(unpair, move |act, _: &Pressed| {
                let c = &act.services.connect;
                let id = selected(act.ui, p.devices)
                    .and_then(nth)
                    .and_then(|i| c.pairing.borrow().devices().get(i).map(|d| d.id.clone()));
                let Some(id) = id else { return };
                let who = by(act);
                if let Err(e) = c.pairing.borrow_mut().unpair(&id, &who) {
                    warn(act.session, forge_ui::tr!("The device was not unpaired"), &e);
                }
                act.want_turn();
            });
        }
        {
            let p = parts.clone();
            pb.on(disconnect, move |act, _: &Pressed| {
                let c = &act.services.connect;
                let s = selected(act.ui, p.sessions)
                    .and_then(nth)
                    .and_then(|i| c.remote.borrow().sessions().get(i).cloned());
                let Some(s) = s else { return };
                c.remote.borrow_mut().disconnect(&s.id);
                c.pairing.borrow().session_event(&s.id, &s.device, "disconnected");
                act.want_turn();
            });
        }
        pb.on(relay, move |act, _: &FeedTicked| act.want_turn());

        let mut feeds = false;
        let mut seen = (u64::MAX, u64::MAX);
        let uncapped = services.connect.faults.latency_uncapped();
        pb.sync(root, move |s| {
            let c = &s.services.connect;
            if !feeds {
                feeds = true;
                s.ui.add_feed(
                    readout,
                    LiveFeed {
                        source: feed.clone() as Arc<dyn forge_ui::LiveSource>,
                        // W2 positive control only: an uncapped live readout.
                        max_hz: if uncapped { 60 } else { crate::LIVE_HZ },
                        self_ui: false,
                    },
                );
                let changes = c.remote.borrow().changes();
                s.ui.add_feed(
                    relay,
                    LiveFeed {
                        source: changes,
                        max_hz: crate::LIVE_HZ,
                        self_ui: false,
                    },
                );
            }
            let rev = (c.remote.borrow().revision(), c.pairing.borrow().revision());
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            refresh(s.ui, s.services, &parts, head, exposure);
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}

fn refresh(
    ui: &mut Ui,
    services: &forge_editor::services::EditorServices,
    p: &Parts,
    head: forge_ui::Signal<String>,
    exposure: forge_ui::Signal<String>,
) {
    let c = &services.connect;
    let t = c.remote.borrow();
    let store = c.pairing.borrow();
    let e = store.exposure();
    set(
        ui,
        head,
        forge_ui::trf!(
            "{devices} paired device(s) \u{b7} {sessions} remote session(s)",
            devices = store.devices().len(),
            sessions = t.sessions().len()
        ),
    );
    set(
        ui,
        exposure,
        forge_ui::trf!(
            "Listening on {address} \u{2014} {label}",
            address = t.address(e),
            label = forge_ui::l10n::tr(e.label())
        ),
    );
    fill(
        ui,
        p.requests,
        t.requests()
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    i as u64 + 1,
                    forge_ui::trf!("\u{201c}{device}\u{201d} asks to pair", device = r.device),
                    false,
                    None,
                )
            })
            .collect(),
    );
    fill(
        ui,
        p.devices,
        store
            .devices()
            .iter()
            .enumerate()
            .map(|(i, d)| {
                (
                    i as u64 + 1,
                    forge_ui::trf!(
                        "{id} \u{201c}{name}\u{201d} \u{b7} allowed by {by}",
                        id = d.id,
                        name = d.name,
                        by = d.allowed_by
                    ),
                    false,
                    None,
                )
            })
            .collect(),
    );
    crate::ui::show_empty(ui, p.sessions_empty, t.sessions().is_empty());
    fill(
        ui,
        p.sessions,
        t.sessions()
            .iter()
            .enumerate()
            .map(|(i, s)| {
                (
                    i as u64 + 1,
                    forge_ui::trf!(
                        "{id} from {device} \u{b7} {n} command(s)",
                        id = s.id,
                        device = s.device,
                        n = s.commands
                    ),
                    false,
                    None,
                )
            })
            .collect(),
    );
}
