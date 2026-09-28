//! **Presence** (`forge.presence`, Ch.37 §37.7; DoD M2-59, M6-13): who is looking at what.
//!
//! Presence is session state each editor publishes for itself (its selection and the panel
//! in focus), read-only to teammates. This panel lists every teammate online with what they
//! have selected, by name; the hierarchy, the inspector and the viewport draw the same thing
//! where the entity is. It is a live feed coalesced to at most 10 Hz, refreshed only while
//! the panel is visible (§21.11): an idle team costs nothing.

use forge_editor::panels::PanelCx;
use forge_ui::widgets::LabelKind;

use crate::ui::{feed_once, fill, frame, list, notice, set, text};

/// A panel id as people read it (`forge.inspector` → `Inspector`).
#[must_use]
pub fn panel_name(id: &str) -> String {
    let last = id.rsplit('.').next().unwrap_or(id).replace('_', " ");
    let mut c = last.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let f = frame(pb, forge_ui::tr!("Presence"))?;
        let root = f.root;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Heading)?;
        let people = list(pb, root, "people", forge_ui::tr!("Teammates online"), 140.0)?;
        text(
            pb,
            root,
            "hint",
            forge_ui::tr!(
                "The hierarchy, the inspector and the viewport show the same thing where you work."
            ),
            LabelKind::Small,
        )?;
        let mut feeds = false;
        let mut seen = u64::MAX;
        pb.sync(root, move |s| {
            feed_once(s, f.relay, &mut feeds, crate::LIVE_HZ);
            let rev = s.services.collab.revision() ^ s.mirror.revision().rotate_left(11);
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            notice(s.ui, s.services, &f);
            let collab = &s.services.collab;
            let me = collab.me();
            let present: Vec<_> = collab
                .server
                .as_ref()
                .map(|v| v.presence())
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.user != me)
                .collect();
            set(
                s.ui,
                head,
                match present.len() {
                    0 => forge_ui::tr!("No teammate is online.").to_string(),
                    1 => forge_ui::tr!("1 teammate online").to_string(),
                    n => forge_ui::trf!("{n} teammates online", n),
                },
            );
            let rows = present
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let names: Vec<String> = p
                        .selection
                        .iter()
                        .map(|k| {
                            s.mirror.entity(forge_cmd::EntityKey(*k)).map_or_else(
                                || format!("e{k}"),
                                |e| format!("\u{201c}{}\u{201d}", e.name),
                            )
                        })
                        .collect();
                    let panel = p.panel.as_deref().map(panel_name).unwrap_or_default();
                    let looking = if names.is_empty() {
                        forge_ui::tr!("nothing selected").to_string()
                    } else {
                        forge_ui::trf!("looking at {names}", names = names.join(", "))
                    };
                    let line = if panel.is_empty() {
                        format!("{} \u{2014} {looking}", p.user)
                    } else {
                        format!("{} \u{2014} {panel} \u{2014} {looking}", p.user)
                    };
                    (i as u64, line, false)
                })
                .collect();
            fill(s.ui, people, rows);
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn panel_names_read_as_words() {
        assert_eq!(super::panel_name("forge.inspector"), "Inspector");
        assert_eq!(super::panel_name("forge.publish_queue"), "Publish queue");
    }
}
