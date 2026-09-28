//! **ViewSpec** — how a WASM plugin declares a panel (Ch.21 §21.19, WP-21).
//!
//! A WASM component cannot hold widgets across the sandbox boundary, so its `EditorPanel`
//! item returns a serialisable tree of catalogue widgets and the host builds it with the same
//! widgets every panel uses. `call("EditorPanel", key, [])` returns JSON:
//!
//! ```json
//! { "title": "Rivers", "dock": "right", "body": [
//!     { "heading": "Rivers" },
//!     { "text": "Raise the water table of the open project." },
//!     { "setting": { "key": "rivers.level", "label": "Level" } },
//!     { "row": [ { "button": { "label": "Raise", "command": "rivers.raise", "args": {} } } ] }
//! ] }
//! ```
//!
//! * `heading` / `text` — labels. `setting` — a live, read-only readout of a project setting
//!   (it follows the mirror; the panel changes nothing by itself).
//! * `button` — emits `Invoke { command, args }` on the bus (I7). **Only a command the same
//!   plugin provides**: a WASM command plans under the plugin's own capability grant (E-27),
//!   so a panel's actions can do nothing its plugin was not granted. A button naming any
//!   other command is shown disabled with the reason — a plugin cannot borrow the person's
//!   authority by labelling a button.
//! * The title and dock are read once, at install; the body at every build of the panel (a
//!   hot reload shows the new body the next time the panel is opened or re-docked). Bounded:
//!   at most [`MAX_NODES`] nodes, [`MAX_DEPTH`] rows deep.
//!
//! Custom-painted widgets are source-plugin only (Ch.32 §32.3's table).

use std::sync::Arc;

use forge_cmd::EditorCommand;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_ui::widgets::{Button, Container, Label, LabelKind, Pressed};
use forge_ui::{NodeStyle, Role, Signal, UiError, WidgetId};
use forge_wasm::{GuestItem, WasmError, WasmHost};
use serde::Deserialize;

use crate::panel_rt::PanelBuilder;
use crate::panels::PanelCx;

/// Nodes one view may hold.
pub const MAX_NODES: usize = 256;
/// Rows one view may nest.
pub const MAX_DEPTH: usize = 4;

/// Where the panel docks by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockSpec {
    Left,
    #[default]
    Right,
    Bottom,
    Center,
    Floating,
}

impl DockSpec {
    fn dock(self) -> Dock {
        match self {
            Self::Left => Dock::Left,
            Self::Right => Dock::Right,
            Self::Bottom => Dock::Bottom,
            Self::Center => Dock::Center,
            Self::Floating => Dock::Floating,
        }
    }
}

/// A button's action: a command of the plugin's own.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonSpec {
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// A setting readout.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingSpec {
    pub key: String,
    pub label: String,
}

/// One node of a view.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewNode {
    Heading(String),
    Text(String),
    Setting(SettingSpec),
    Button(ButtonSpec),
    Row(Vec<ViewNode>),
}

/// A plugin's panel, as it declares it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewSpec {
    pub title: String,
    #[serde(default)]
    pub dock: DockSpec,
    #[serde(default)]
    pub body: Vec<ViewNode>,
}

impl ViewSpec {
    /// Parse and bound a view.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let v: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if v.title.trim().is_empty() || v.title.chars().count() > 64 {
            return Err("the title must be 1-64 characters".into());
        }
        fn walk(nodes: &[ViewNode], depth: usize, n: &mut usize) -> Result<(), String> {
            if depth > MAX_DEPTH {
                return Err(format!("rows nest deeper than {MAX_DEPTH}"));
            }
            for node in nodes {
                *n += 1;
                if *n > MAX_NODES {
                    return Err(format!("more than {MAX_NODES} nodes"));
                }
                if let ViewNode::Row(kids) = node {
                    walk(kids, depth + 1, n)?;
                }
            }
            Ok(())
        }
        walk(&v.body, 0, &mut 0)?;
        Ok(v)
    }
}

fn fetch(item: &GuestItem) -> Result<ViewSpec, WasmError> {
    let out = item.call(&[])?;
    ViewSpec::parse(&out).map_err(|why| WasmError::BadOutput {
        plugin: item.plugin().to_string(),
        item: item.name(),
        why,
    })
}

/// Let WASM plugins provide and replace `EditorPanel` items through `host` (the editor's
/// point; the editor registers this on its host).
pub fn adapt_panels(host: &mut WasmHost) {
    host.adapt::<EditorPanel<PanelCx>>(|item| {
        let first = fetch(&item)?;
        Ok(descriptor(item, &first))
    });
}

fn descriptor(item: GuestItem, first: &ViewSpec) -> PanelDescriptor<PanelCx> {
    PanelDescriptor {
        title: first.title.clone(),
        icon: None,
        default_dock: first.dock.dock(),
        build: Arc::new(move |cx: &mut PanelCx| {
            let item = item.clone();
            match fetch(&item) {
                Ok(view) => cx.add_live(move |pb| build_view(pb, &item, &view)),
                Err(e) => {
                    let msg = e.to_string();
                    cx.add(move |b, parent| {
                        b.add(
                            parent,
                            "wasm_error",
                            NodeStyle::leaf(),
                            Label::new(msg.as_str()).kind(LabelKind::Body).wrapping(),
                        )?;
                        Ok(())
                    });
                    cx.empty_state(forge_ui::tr!(
                        "This plugin's panel did not build; fix the plugin and reopen the panel."
                    ));
                }
            }
        }),
    }
}

/// Why `command` may not be a button of `item`'s plugin (`None`: it may).
fn refused(pb: &PanelBuilder, item: &GuestItem, command: &str) -> Option<String> {
    let owner = pb.services().hosting.borrow().command_owner(command);
    match owner {
        Some(o) if o == item.plugin().as_str() => None,
        Some(o) => Some(format!(
            "\u{26a0} {command} belongs to {o}: a plugin's panel runs only its own commands"
        )),
        None => Some(format!(
            "\u{26a0} {command} is not a command of {}",
            item.plugin()
        )),
    }
}

fn setting_text(pb_or_mirror: Option<&forge_cmd::Value>, label: &str) -> String {
    match pb_or_mirror {
        Some(v) => format!("{label}: {v}"),
        None => format!("{label}: (not set)"),
    }
}

fn build_nodes(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    item: &GuestItem,
    nodes: &[ViewNode],
    path: &str,
    readouts: &mut Vec<(String, String, Signal<String>)>,
) -> Result<(), UiError> {
    let space = pb.b.theme_ref().space;
    for (i, node) in nodes.iter().enumerate() {
        let key = forge_ui::Key::Str(Arc::from(format!("{path}{i}").as_str()));
        match node {
            ViewNode::Heading(t) => {
                pb.b.add(
                    parent,
                    key,
                    NodeStyle::leaf().padding(space[1]),
                    Label::new(t.as_str()).kind(LabelKind::Heading),
                )?;
            }
            ViewNode::Text(t) => {
                pb.b.add(
                    parent,
                    key,
                    NodeStyle::leaf().padding(space[1]),
                    Label::new(t.as_str()).kind(LabelKind::Body).wrapping(),
                )?;
            }
            ViewNode::Setting(s) => {
                let now = setting_text(pb.mirror().setting(&s.key), &s.label);
                let sig = pb.b.signal(now);
                pb.b.add(
                    parent,
                    key,
                    NodeStyle::leaf().padding(space[1]),
                    Label::new(sig).kind(LabelKind::Body),
                )?;
                readouts.push((s.key.clone(), s.label.clone(), sig));
            }
            ViewNode::Button(b) => match refused(pb, item, &b.command) {
                None => {
                    let id = pb.b.add(
                        parent,
                        key,
                        NodeStyle::leaf(),
                        Button::new(b.label.as_str()),
                    )?;
                    let (target, args) = (b.command.clone(), b.args.to_string());
                    pb.on(id, move |act, _: &Pressed| {
                        act.cmd.emit(EditorCommand::Invoke {
                            target: target.clone(),
                            args: args.clone(),
                        });
                    });
                }
                Some(why) => {
                    // No button at all: nothing to press that could run someone else's
                    // command.
                    pb.b.add(
                        parent,
                        key,
                        NodeStyle::leaf().padding(space[1]),
                        Label::new(format!("{} \u{2014} {why}", b.label))
                            .kind(LabelKind::Small)
                            .wrapping(),
                    )?;
                }
            },
            ViewNode::Row(kids) => {
                let row = pb.b.add(
                    parent,
                    key.clone(),
                    NodeStyle::row(space[1]).padding(space[1]),
                    Container::new(Role::Group),
                )?;
                build_nodes(pb, row, item, kids, &format!("{path}{i}."), readouts)?;
            }
        }
    }
    Ok(())
}

fn build_view(pb: &mut PanelBuilder, item: &GuestItem, view: &ViewSpec) -> Result<(), UiError> {
    let space = pb.b.theme_ref().space;
    let root = pb.b.add(
        pb.parent,
        "content",
        NodeStyle::column(space[1])
            .padding(space[2])
            .grow(1.0)
            .scrollable(),
        Container::new(Role::Group).labelled(&view.title),
    )?;
    let mut readouts = Vec::new();
    build_nodes(pb, root, item, &view.body, "n", &mut readouts)?;
    pb.b.add(
        root,
        "footer",
        NodeStyle::leaf().padding(space[1]),
        Label::new(forge_ui::trf!(
            "From {plugin} (WASM, sandboxed): its buttons run its own commands under its grants.",
            plugin = item.plugin()
        ))
        .kind(LabelKind::Small)
        .wrapping(),
    )?;
    if !readouts.is_empty() {
        let mut seen = u64::MAX;
        pb.sync(root, move |s| {
            let rev = s.mirror.settings_revision();
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            for (key, label, sig) in &readouts {
                let v = setting_text(s.mirror.setting(key), label);
                if sig.get(s.ui.rt()) != v {
                    sig.set(s.ui.rt_mut(), v);
                }
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_parse_and_are_bounded() {
        let v = ViewSpec::parse(
            br#"{"title":"Rivers","body":[{"heading":"Rivers"},{"setting":{"key":"a","label":"A"}},
                {"row":[{"button":{"label":"Go","command":"rivers.go"}}]}]}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(v.dock, DockSpec::Right);
        assert_eq!(v.body.len(), 3);
        assert!(ViewSpec::parse(br#"{"title":"","body":[]}"#).is_err());
        assert!(ViewSpec::parse(br#"{"title":"x","body":[{"paint":1}]}"#).is_err());
        let deep = format!(
            r#"{{"title":"x","body":[{}{}]}}"#,
            r#"{"row":["#.repeat(6),
            "]}".repeat(6)
        );
        assert!(ViewSpec::parse(deep.as_bytes()).is_err());
        let many = format!(
            r#"{{"title":"x","body":[{}]}}"#,
            vec![r#"{"text":"t"}"#; MAX_NODES + 1].join(",")
        );
        assert!(ViewSpec::parse(many.as_bytes()).is_err());
    }
}
