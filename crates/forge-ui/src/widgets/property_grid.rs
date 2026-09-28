//! The property grid (§21.16, the §21.21 inspector's body): rows of *name | editor*
//! grouped in collapsible categories, with a filter.
//!
//! Rows are **data-driven**: each [`Property`] names its editor through [`PropKind`], and
//! the grid builds the matching catalogue widget (checkbox, numeric field with units,
//! text field, combo, colour button, vector/quaternion editor, asset picker, curve
//! editor, read-only text). The inspector (WP-U5) maps `forge-reflect`'s inspector
//! metadata (Ch.6) onto `PropKind`; nothing here knows about reflection, so games use the
//! same grid for settings screens.
//!
//! Every editor is a real widget: Tab walks the rows, each editor keeps its own keyboard
//! behaviour, and each row's name labels its editor for assistive technology. Typing in
//! the filter hides rows whose name does not match (fuzzy) and categories left empty;
//! hidden rows lay out and paint nothing (§21.3).

use std::cell::RefCell;
use std::rc::Rc;

use accesskit::Role;
use forge_frames::DQuat;

use crate::UiError;
use crate::color::HdrColor;
use crate::damage::Dirty;
use crate::fuzzy::fuzzy_match;
use crate::id::{Key, WidgetId};
use crate::input::{Handled, UiEvent};
use crate::layout::NodeStyle;
use crate::state::Signal;
use crate::widget::{A11yCx, Binder, EventCx, PaintCx, Widget};

use super::{
    AssetIndex, AssetRefPicker, Build, Checkbox, CollapsibleHeader, ColorButton, ComboBox,
    Container, Curve, CurveEditor, Label, LabelKind, NumericField, SearchField, TextField,
    quat_editor, vector_editor,
};

/// Which editor a row uses, and the signal it edits.
pub enum PropKind {
    Bool(Signal<bool>),
    Number {
        value: Signal<f64>,
        unit: Option<String>,
        range: Option<(f64, f64)>,
    },
    Text(Signal<String>),
    Choice {
        options: Vec<String>,
        selected: Signal<usize>,
    },
    Color(Signal<HdrColor>),
    Vec3 {
        value: Signal<[f64; 3]>,
        unit: Option<String>,
    },
    Rotation(Signal<DQuat>),
    Asset {
        value: Signal<Option<String>>,
        kind: String,
        index: Rc<dyn AssetIndex>,
    },
    Curve(Signal<Curve>),
    /// Shown, not editable (computed values, ids).
    ReadOnly(Signal<String>),
}

/// One row.
pub struct Property {
    pub name: String,
    pub category: String,
    pub kind: PropKind,
}

impl Property {
    pub fn new(category: &str, name: &str, kind: PropKind) -> Self {
        Self {
            name: name.to_string(),
            category: category.to_string(),
            kind,
        }
    }
}

/// The ids a property grid is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyGridParts {
    pub grid: WidgetId,
    pub filter: WidgetId,
    /// `(name, row, editor)` in order.
    pub rows: Vec<(String, WidgetId, WidgetId)>,
    /// `(category, header, body)`.
    pub categories: Vec<(String, WidgetId, WidgetId)>,
}

#[derive(Default)]
struct Rows {
    rows: Vec<(String, WidgetId, usize)>,
    cats: Vec<(WidgetId, WidgetId)>,
}

const NAME_W: f32 = 120.0;

/// Build a property grid under `parent`.
pub fn property_grid(
    b: &mut dyn Build,
    parent: WidgetId,
    key: impl Into<Key>,
    label: &str,
    props: Vec<Property>,
) -> Result<PropertyGridParts, UiError> {
    let space = b.theme_ref().space;
    let filter_sig = b.signal(String::new());
    let shared = Rc::new(RefCell::new(Rows::default()));
    let grid = b.add(
        parent,
        key,
        NodeStyle::column(space[1]),
        PropertyGrid {
            label: label.to_string(),
            filter: filter_sig,
            rows: shared.clone(),
        },
    )?;
    let filter = b.add(
        grid,
        "filter",
        NodeStyle::leaf().width(NAME_W + 240.0),
        SearchField::new(filter_sig, &crate::trf!("Filter {label}", label)),
    )?;
    let mut parts = PropertyGridParts {
        grid,
        filter,
        rows: Vec::new(),
        categories: Vec::new(),
    };
    let mut cat_order: Vec<String> = Vec::new();
    for p in &props {
        if !cat_order.contains(&p.category) {
            cat_order.push(p.category.clone());
        }
    }
    let mut props: Vec<Option<Property>> = props.into_iter().map(Some).collect();
    for (ci, cat) in cat_order.iter().enumerate() {
        let open = b.signal(true);
        let body_key = Key::Str(format!("body-{ci}").into());
        let hkey = Key::Str(format!("head-{ci}").into());
        let body_id = grid.child(&body_key);
        let mut header = CollapsibleHeader::new(cat, open);
        header.set_body(body_id);
        let head = b.add(grid, hkey, NodeStyle::leaf().width(NAME_W + 240.0), header)?;
        let body = b.add(
            grid,
            body_key,
            NodeStyle::column(space[1]).padding(space[1]),
            Container::new(Role::Group).labelled(cat),
        )?;
        let ci_idx = shared.borrow().cats.len();
        shared.borrow_mut().cats.push((head, body));
        parts.categories.push((cat.clone(), head, body));
        for (ri, slot) in props.iter_mut().enumerate() {
            if slot.as_ref().is_none_or(|p| &p.category != cat) {
                continue;
            }
            let Some(p) = slot.take() else { continue };
            let row = b.add(
                body,
                Key::Index(ri as u32),
                NodeStyle::row(space[2]),
                Container::new(Role::Group).labelled(&p.name),
            )?;
            b.add(
                row,
                "name",
                NodeStyle::leaf().width(NAME_W),
                Label::new(p.name.as_str()).kind(LabelKind::Muted),
            )?;
            let ed = build_editor(b, row, &p)?;
            shared.borrow_mut().rows.push((p.name.clone(), row, ci_idx));
            parts.rows.push((p.name, row, ed));
        }
    }
    Ok(parts)
}

fn build_editor(b: &mut dyn Build, row: WidgetId, p: &Property) -> Result<WidgetId, UiError> {
    let name = p.name.as_str();
    let w = NodeStyle::leaf().width(240.0);
    match &p.kind {
        PropKind::Bool(s) => b.add(row, "ed", NodeStyle::leaf(), Checkbox::new(*s, name)),
        PropKind::Number { value, unit, range } => {
            let mut f = NumericField::new(*value, name);
            if let Some(u) = unit {
                f = f.unit(u);
            }
            if let Some((lo, hi)) = range {
                f = f.range(*lo, *hi);
            }
            b.add(row, "ed", NodeStyle::leaf().width(140.0), f)
        }
        PropKind::Text(s) => b.add(row, "ed", w, TextField::new(*s, name)),
        PropKind::Choice { options, selected } => {
            let opts: Vec<&str> = options.iter().map(String::as_str).collect();
            b.add(row, "ed", w, ComboBox::new(name, &opts, *selected))
        }
        PropKind::Color(s) => b.add(
            row,
            "ed",
            NodeStyle::leaf().width(80.0),
            ColorButton::new(*s, name),
        ),
        PropKind::Vec3 { value, unit } => {
            Ok(vector_editor::<3>(b, row, "ed", *value, name, unit.as_deref())?.editor)
        }
        PropKind::Rotation(s) => Ok(quat_editor(b, row, "ed", *s, name)?.editor),
        PropKind::Asset { value, kind, index } => {
            let choice = b.signal(None);
            b.add(
                row,
                "ed",
                w,
                AssetRefPicker::new(*value, choice, kind, index.clone(), name),
            )
        }
        PropKind::Curve(s) => b.add(
            row,
            "ed",
            NodeStyle::leaf().size(240.0, 100.0),
            CurveEditor::new(*s, name),
        ),
        PropKind::ReadOnly(s) => b.add(row, "ed", NodeStyle::leaf(), Label::new(*s)),
    }
}

/// The property grid's root: applies the filter.
pub struct PropertyGrid {
    label: String,
    filter: Signal<String>,
    rows: Rc<RefCell<Rows>>,
}

impl PropertyGrid {
    /// Rows matching `query` (empty: all).
    fn matches(query: &str, name: &str) -> bool {
        query.trim().is_empty() || fuzzy_match(query, name).is_some()
    }
}

impl Widget for PropertyGrid {
    fn role(&self) -> Role {
        Role::Group
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.filter.any()), Dirty::NONE);
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if *ev != UiEvent::BindingChanged {
            return Handled::No;
        }
        let q = self.filter.get(cx.rt());
        let rows = self.rows.borrow();
        let mut visible = vec![0usize; rows.cats.len()];
        for (name, row, cat) in &rows.rows {
            let show = Self::matches(&q, name);
            cx.set_hidden(*row, !show);
            if show && let Some(v) = visible.get_mut(*cat) {
                *v += 1;
            }
        }
        for (i, (head, body)) in rows.cats.iter().enumerate() {
            let empty = visible.get(i).copied().unwrap_or(0) == 0;
            cx.set_hidden(*head, empty);
            if empty {
                cx.set_hidden(*body, true);
            } else if !q.trim().is_empty() {
                // A match inside a collapsed category is shown.
                cx.set_hidden(*body, false);
            }
        }
        Handled::Yes
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
    }
}
