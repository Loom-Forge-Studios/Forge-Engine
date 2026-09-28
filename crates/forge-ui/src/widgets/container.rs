use accesskit::Role;

use crate::widget::{A11yCx, PaintCx, Widget};

/// A layout container (a panel, a group, a row). Its background, if any, comes from its
/// `NodeStyle::background` token; children lay out by taffy.
pub struct Container {
    role: Role,
    label: Option<String>,
}

impl Container {
    pub fn new(role: Role) -> Self {
        Self { role, label: None }
    }
    /// A generic grouping.
    pub fn group() -> Self {
        Self::new(Role::Group)
    }
    /// A named panel (a focus scope usually).
    pub fn pane(label: &str) -> Self {
        Self {
            role: Role::Pane,
            label: Some(label.to_string()),
        }
    }
    pub fn labelled(mut self, label: &str) -> Self {
        self.label = Some(label.to_string());
        self
    }
}

impl Widget for Container {
    fn role(&self) -> Role {
        self.role
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        if let Some(l) = &self.label {
            node.set_label(l.as_str());
        }
    }
}
