//! Accessibility: the AccessKit tree (Ch.21 §21.10).
//!
//! * Every widget contributes a node: its role (required by `Widget`), bounds, children,
//!   and whatever `Widget::a11y` adds (name, value, range, states, actions).
//! * **Incremental.** Step 7 of §21.3 emits a `TreeUpdate` with only the nodes whose
//!   accessible properties changed (compared against the last sent node).
//! * **Lazily activated.** Nothing is built until an assistive technology connects
//!   ([`Ui::a11y_activate`], called from `accesskit_winit`'s activation handler), so the
//!   cost is zero for users who do not use one.
//! * The node id is the `WidgetId`, so a screen reader keeps its place across updates.

use std::collections::HashMap;

use accesskit::{Action, ActionRequest, Node, NodeId, TreeId, TreeInfo, TreeUpdate};

use crate::damage::Dirty;
use crate::id::WidgetId;
use crate::input::UiEvent;
use crate::ui::{NodeKey, Ui};
use crate::widget::A11yCx;

#[derive(Default)]
pub struct A11yState {
    pub(crate) active: bool,
    sent: HashMap<WidgetId, Node>,
    pending: Option<TreeUpdate>,
    pub(crate) focus_changed: bool,
    /// Nodes built since activation (the lazy-activation test reads it).
    pub nodes_built: u64,
    /// Virtual child node → (owning widget, local key): routes AT actions to the owner.
    pub(crate) virtual_owner: HashMap<WidgetId, (WidgetId, u64)>,
}

/// A virtual accessibility child (`Widget::a11y_children`).
pub(crate) struct VirtualNode {
    id: WidgetId,
    node: Node,
    local: u64,
}

impl A11yState {
    pub(crate) fn forget(&mut self, id: WidgetId) {
        self.sent.remove(&id);
    }
}

impl Ui {
    /// A widget's node plus its virtual children `(id, node, local key)` (§21.10).
    fn build_a11y_node(&self, k: NodeKey) -> Option<(Node, Vec<VirtualNode>)> {
        let n = self.nodes.get(k)?;
        let w = n.widget.as_ref()?;
        let mut node = Node::new(w.role());
        let s = self.scale_factor() as f64;
        node.set_bounds(accesskit::Rect {
            x0: f64::from(n.rect.x) * s,
            y0: f64::from(n.rect.y) * s,
            x1: f64::from(n.rect.right()) * s,
            y1: f64::from(n.rect.bottom()) * s,
        });
        let mut kids: Vec<NodeId> = n
            .children
            .iter()
            .filter(|c| !self.nodes[**c].hidden)
            .map(|c| self.nodes[*c].id.to_accesskit())
            .collect();
        if w.focusable() {
            node.add_action(Action::Focus);
        }
        let cx = A11yCx {
            rt: &self.rt,
            state: n.state,
            rect: n.rect,
            scale: s as f32,
        };
        w.a11y(&cx, &mut node);
        // A showing tooltip describes its owner (WAI-ARIA `aria-describedby`): the owner
        // points at the bubble and carries its text as the description unless the widget
        // set one itself.
        if let Some((owner, bubble)) = self.tooltip
            && owner == n.id
        {
            node.set_described_by(vec![bubble.to_accesskit()]);
            if node.description().is_none()
                && let Some(text) = w.tooltip(&self.rt)
            {
                node.set_description(text);
            }
        }
        let mut virt = Vec::new();
        w.a11y_children(&cx, &mut virt);
        let mut out = Vec::with_capacity(virt.len());
        for (local, vn) in virt {
            let id = n.id.child(&crate::id::Key::Id(local));
            kids.push(id.to_accesskit());
            out.push(VirtualNode {
                id,
                node: vn,
                local,
            });
        }
        if !kids.is_empty() {
            node.set_children(kids);
        }
        Some((node, out))
    }

    /// Record a widget's virtual children as sent; returns those that changed.
    fn take_virtual(&mut self, owner: WidgetId, virt: Vec<VirtualNode>) -> Vec<(NodeId, Node)> {
        let mut changed = Vec::new();
        for v in virt {
            self.a11y.virtual_owner.insert(v.id, (owner, v.local));
            if self.a11y.sent.get(&v.id) != Some(&v.node) {
                self.a11y.sent.insert(v.id, v.node.clone());
                changed.push((v.id.to_accesskit(), v.node));
            }
        }
        changed
    }

    fn focus_node(&self) -> NodeId {
        let Some(f) = self.focused else {
            return WidgetId::ROOT.to_accesskit();
        };
        let local = self
            .index
            .get(&f)
            .and_then(|k| self.nodes[*k].widget.as_ref())
            .and_then(|w| w.a11y_focus(&self.rt));
        match local {
            Some(l) => f.child(&crate::id::Key::Id(l)).to_accesskit(),
            None => f.to_accesskit(),
        }
    }

    /// Step 7: queue a `TreeUpdate` with the changed nodes (only while active).
    pub(crate) fn update_a11y(&mut self) {
        if !self.a11y.active {
            return;
        }
        let keys: Vec<NodeKey> = self
            .dirty_keys_snapshot()
            .into_iter()
            .filter(|k| {
                self.nodes
                    .get(*k)
                    .is_some_and(|n| n.dirty.intersects(Dirty::A11Y))
                    && self.is_visible_key(*k)
            })
            .collect();
        let mut changed = Vec::new();
        for k in keys {
            let Some((node, virt)) = self.build_a11y_node(k) else {
                continue;
            };
            self.a11y.nodes_built += 1;
            let id = self.nodes[k].id;
            if self.a11y.sent.get(&id) != Some(&node) {
                self.a11y.sent.insert(id, node.clone());
                changed.push((id.to_accesskit(), node));
            }
            let mut v = self.take_virtual(id, virt);
            changed.append(&mut v);
        }
        if changed.is_empty() && !self.a11y.focus_changed {
            return;
        }
        self.a11y.focus_changed = false;
        let focus = self.focus_node();
        match self.a11y.pending.as_mut() {
            Some(p) => {
                p.nodes.extend(changed);
                p.focus = focus;
            }
            None => {
                self.a11y.pending = Some(TreeUpdate {
                    nodes: changed,
                    tree: None,
                    tree_id: TreeId::ROOT,
                    focus,
                });
            }
        }
    }

    /// An assistive technology connected: build and return the full tree. From now on,
    /// every frame queues incremental updates.
    pub fn a11y_activate(&mut self) -> TreeUpdate {
        self.a11y.active = true;
        self.a11y.focus_changed = false;
        self.a11y.pending = None;
        let mut nodes = Vec::new();
        for k in self.paint_order() {
            if let Some((node, virt)) = self.build_a11y_node(k) {
                self.a11y.nodes_built += 1;
                let id = self.nodes[k].id;
                self.a11y.sent.insert(id, node.clone());
                nodes.push((id.to_accesskit(), node));
                let mut v = self.take_virtual(id, virt);
                nodes.append(&mut v);
            }
        }
        TreeUpdate {
            nodes,
            tree: Some(TreeInfo {
                root: WidgetId::ROOT.to_accesskit(),
                toolkit_name: Some("forge-ui".into()),
                toolkit_version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            tree_id: TreeId::ROOT,
            focus: self.focus_node(),
        }
    }

    /// The assistive technology went away: stop building updates.
    pub fn a11y_deactivate(&mut self) {
        self.a11y.active = false;
        self.a11y.sent.clear();
        self.a11y.virtual_owner.clear();
        self.a11y.pending = None;
    }

    pub fn a11y_active(&self) -> bool {
        self.a11y.active
    }
    pub fn a11y_nodes_built(&self) -> u64 {
        self.a11y.nodes_built
    }

    /// The incremental update queued by the last frames, if any.
    pub fn a11y_take_update(&mut self) -> Option<TreeUpdate> {
        self.a11y.pending.take()
    }

    /// The last node sent for a widget (tests and audits read the tree through this).
    pub fn a11y_node(&self, id: WidgetId) -> Option<&Node> {
        self.a11y.sent.get(&id)
    }

    /// The virtual children (`Widget::a11y_children`: the live rows of a virtualised tree,
    /// list or grid, the options of a radio group, …) the last node sent for `id` lists, in
    /// order. Read their nodes with [`Ui::a11y_node`]. Rows that scrolled away are not
    /// listed, even though their last node is still remembered.
    pub fn a11y_virtual_children(&self, id: WidgetId) -> Vec<WidgetId> {
        let Some(n) = self.a11y.sent.get(&id) else {
            return Vec::new();
        };
        n.children()
            .iter()
            .map(|c| WidgetId(c.0))
            .filter(|c| {
                self.a11y
                    .virtual_owner
                    .get(c)
                    .is_some_and(|(owner, _)| *owner == id)
            })
            .collect()
    }

    /// An action requested by an assistive technology.
    pub fn a11y_action(&mut self, req: &ActionRequest) {
        let id = WidgetId(req.target_node.0);
        if let Some((owner, local)) = self.a11y.virtual_owner.get(&id).copied() {
            self.send(owner, &UiEvent::A11yChildAction(local, req.action));
            self.flush_pending_close();
            return;
        }
        match req.action {
            Action::Focus => self.set_focus(Some(id), true),
            other => {
                self.send(id, &UiEvent::A11yAction(other));
            }
        }
    }

    fn dirty_keys_snapshot(&self) -> Vec<NodeKey> {
        self.dirty_keys_ref().to_vec()
    }
}
