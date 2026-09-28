//! **Licence** (`forge.licence`, Ch.38 §38.2, E-57, I21; DoD M2-62): tier, seat, bound team,
//! expiry and fallback, **evaluated on this machine with no network call** (the status never
//! reaches for the network; only *Renew*, which a person presses, may).
//!
//! **On lapse: a banner and a Renew button; the collaboration panels grey; nothing is
//! locked** — every project still opens, edits, saves, builds and ships (E-57). Renewal is
//! opportunistic, never a precondition for anything.

use forge_editor::collab::licence::{EntitlementSource, date};
use forge_editor::panels::PanelCx;
use forge_ui::widgets::{LabelKind, Pressed};

use crate::ui::{bar, feed_once, frame, group, primary, set, show, text};

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("the licence status of this machine"));
    cx.add_live(|pb| {
        let f = frame(pb, forge_ui::tr!("Licence"))?;
        let root = f.root;
        let banner_group = group(pb, root, "banner_group", forge_ui::tr!("Licence"))?;
        let banner = pb.b.signal(String::new());
        text(pb, banner_group, "banner", banner, LabelKind::Warning)?;
        let rbar = bar(pb, banner_group, "renew_bar", forge_ui::tr!("Renew"))?;
        let renew = primary(pb, rbar, "renew", forge_ui::tr!("Renew\u{2026}"))?;
        let lines = pb.b.signal(String::new());
        text(pb, root, "details", lines, LabelKind::Body)?;
        let grace = pb.b.signal(String::new());
        let grace_label = text(pb, root, "grace", grace, LabelKind::Muted)?;
        let footer = pb.b.signal(String::new());
        text(pb, root, "footer", footer, LabelKind::Small)?;
        text(pb, root, "status", f.status, LabelKind::Muted)?;
        pb.on(renew, move |act, _: &Pressed| {
            let msg = act.services.collab.licence.renew();
            set(act.ui, f.status, msg);
            act.want_turn();
        });

        let mut feeds = false;
        let mut seen = u64::MAX;
        pb.sync(root, move |s| {
            feed_once(s, f.relay, &mut feeds, crate::LIVE_HZ);
            let rev = s.services.collab.revision();
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            let st = s.services.collab.licence_state();
            let team = s
                .services
                .collab
                .status()
                .and_then(|x| x.team_name.clone().or(x.team.clone()));
            let details = match &st.entitlement {
                None => forge_ui::tr!("No licence is activated on this machine.").to_string(),
                Some(e) => forge_ui::trf!(
                    "Tier: {tier} \u{b7} Seat: {seat} \u{b7} Bound team: {team} \u{b7} Expires: {expires}{fallback}",
                    tier = e.tier.label(),
                    seat = e.seat.label(),
                    team = e.bound.as_deref().unwrap_or(forge_ui::tr!("none")),
                    expires = e
                        .expires_ms
                        .map_or_else(|| forge_ui::tr!("never (perpetual)").into(), date),
                    fallback = e
                        .fallback
                        .as_ref()
                        .map(|v| forge_ui::trf!(" \u{b7} Perpetual Team rights up to {v}", v))
                        .unwrap_or_default()
                ),
            };
            set(s.ui, lines, details);
            let lapsed_or_off = !st.collaboration;
            set(
                s.ui,
                banner,
                if lapsed_or_off {
                    format!(
                        "{}.{}",
                        st.why_not.clone().unwrap_or_default(),
                        team.map(|t| forge_ui::trf!(" This project's team: {t}.", t))
                            .unwrap_or_default()
                    )
                } else {
                    String::new()
                },
            );
            show(s.ui, banner_group, lapsed_or_off);
            show(s.ui, rbar, st.lapsed);
            match st.grace_until_ms {
                Some(g) => set(
                    s.ui,
                    grace,
                    forge_ui::trf!("The Team term ended; collaboration continues until {date} (grace period), then pauses until renewed.", date = date(g)),
                ),
                None => set(s.ui, grace, String::new()),
            }
            show(s.ui, grace_label, st.grace_until_ms.is_some());
            let src: &dyn EntitlementSource = s.services.collab.licence.as_ref();
            set(
                s.ui,
                footer,
                forge_ui::trf!("Checked on this machine, offline (no network call). Entitlement source: {backend}. Building and shipping never depend on it.", backend = src.backend()),
            );
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}
