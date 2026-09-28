//! The **Command palette** panel (`forge.command_palette`, DoD M2-45): the palette itself
//! is the shell's popup (Ctrl+Shift+P, WP-U3's model over every action, panel and
//! command); this panel is its dockable home: how to open it, and a button that does.

use forge_editor::panels::PanelCx;
use forge_editor::session::ShellRequest;
use forge_ui::NodeStyle;
use forge_ui::widgets::{Button, Label, LabelKind, Pressed};

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("the button that opens the palette"));
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let chord = pb
            .session()
            .keymap()
            .borrow()
            .chords_for("forge.palette.open")
            .first()
            .map_or(forge_ui::tr!("the palette action").to_string(), |c| {
                c.label()
            });
        pb.b.add(
            pb.parent,
            "about",
            NodeStyle::leaf().padding(space[2]),
            Label::new(
                forge_ui::trf!(
                    "Every action, panel and command, by keyboard. Press {chord} anywhere and \
                     type a few letters; recently used entries come first.",
                    chord
                )
                .as_str(),
            )
            .kind(LabelKind::Muted)
            .wrapping(),
        )?;
        let open = pb.b.add(
            pb.parent,
            "open",
            NodeStyle::leaf().padding(space[2]),
            Button::new(forge_ui::tr!("Open the command palette")),
        )?;
        pb.on(open, |act, _: &Pressed| {
            act.session
                .request(ShellRequest::RunAction("forge.palette.open".into()));
        });
        Ok(())
    });
}
