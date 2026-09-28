//! Popovers and modal dialogs (§21.16).
//!
//! * A **popover** is a non-modal panel anchored to its button: it takes focus, Tab stays
//!   inside it, and an outside click or Escape closes it.
//! * A **modal dialog** sits on a scrim and traps focus (`FocusScope::Modal`). Escape and
//!   the cancel button close it without acting. A **destructive confirmation must name
//!   the loss** — [`DialogSpec::destructive`] will not build without one — and the loss is
//!   shown and announced (as the dialog's description) before the user can confirm.
//! * Buttons are real `Button` widgets, so Tab, Enter, Space and screen readers work on
//!   them as everywhere else. The confirm button is focused first unless the action is
//!   destructive, in which case the **cancel** button is (the safe default).

use accesskit::Role;

use crate::UiError;
use crate::geom::{Point, Size};
use crate::id::{Key, WidgetId};
use crate::input::{Handled, KeyCode, UiEvent};
use crate::layout::NodeStyle;
use crate::overlay::{Anchor, PopupSpec};
use crate::state::Bind;
use crate::style::{ColorRole, Variant};
use crate::ui::Ui;
use crate::widget::{A11yCx, EventCx, MeasureCx, PaintCx, Widget};

use super::{Button, Label, body, control_pad_x};
use crate::style::button_style as button_style_for;

/// Raised when a dialog is answered. `confirmed: false` for cancel / Escape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogResult {
    pub dialog: String,
    pub confirmed: bool,
}

/// What a dialog says and offers.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogSpec {
    /// A stable id the answer carries (`"delete-entities"`).
    pub id: String,
    pub title: String,
    pub message: String,
    /// For a lossy action: exactly what will be lost.
    pub loss: Option<String>,
    pub confirm: String,
    pub cancel: String,
    pub destructive: bool,
}

impl DialogSpec {
    /// An informational / confirming dialog.
    pub fn confirm(id: &str, title: &str, message: &str, confirm: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            message: message.into(),
            loss: None,
            confirm: confirm.into(),
            cancel: crate::tr!("Cancel").into(),
            destructive: false,
        }
    }
    /// A destructive confirmation. `loss` names what is lost; an empty one is refused.
    pub fn destructive(id: &str, title: &str, loss: &str, confirm: &str) -> Result<Self, UiError> {
        if loss.trim().is_empty() {
            return Err(UiError::Layout(format!(
                "dialog {id}: a destructive confirmation must name the loss"
            )));
        }
        Ok(Self {
            id: id.into(),
            title: title.into(),
            message: String::new(),
            loss: Some(loss.into()),
            confirm: confirm.into(),
            cancel: crate::tr!("Cancel").into(),
            destructive: true,
        })
    }
}

/// The dialog body (a focus-trapping container; its children are the parts).
struct DialogBox {
    spec: DialogSpec,
}

impl Widget for DialogBox {
    fn role(&self) -> Role {
        if self.spec.destructive {
            Role::AlertDialog
        } else {
            Role::Dialog
        }
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if let UiEvent::Key(k) = ev
            && k.pressed
            && k.code == KeyCode::Escape
        {
            cx.action(DialogResult {
                dialog: self.spec.id.clone(),
                confirmed: false,
            });
            cx.close_own_popup();
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let shadow = cx.theme().shadows[2];
        let radius = cx.theme().radius.lg;
        cx.panel(
            r,
            Some(ColorRole::BgRaised),
            Some(ColorRole::Border),
            radius,
            Some(shadow),
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.spec.title.as_str());
        let desc = match &self.spec.loss {
            Some(l) => format!("{} {l}", self.spec.message).trim().to_string(),
            None => self.spec.message.clone(),
        };
        if !desc.is_empty() {
            node.set_description(desc);
        }
        node.set_modal();
    }
}

/// Open a modal dialog owned by `owner`. Returns the dialog widget; the answer arrives as
/// a [`DialogResult`] action (bubbling from the dialog to the application).
pub fn open_dialog(ui: &mut Ui, owner: WidgetId, spec: DialogSpec) -> Result<WidgetId, UiError> {
    let space = ui.theme().space;
    let id = ui.open_popup(
        owner,
        Key::Str(spec.id.as_str().into()),
        PopupSpec::dialog(),
        DialogBox { spec: spec.clone() },
    )?;
    let col = NodeStyle::column(space[3])
        .padding(space[5])
        .min_size(360.0, 0.0);
    ui.set_style(id, col)?;
    ui.add(
        id,
        "title",
        NodeStyle::leaf(),
        Label::new(spec.title.as_str()).heading(),
    )?;
    if !spec.message.is_empty() {
        ui.add(
            id,
            "message",
            NodeStyle::leaf().width(420.0),
            Label::new(spec.message.as_str()).wrapping(),
        )?;
    }
    if let Some(loss) = &spec.loss {
        ui.add(
            id,
            "loss",
            NodeStyle::leaf().width(420.0),
            Label::new(crate::trf!("This cannot be undone: {loss}", loss)).wrapping(),
        )?;
    }
    let mut row = NodeStyle::row(space[2]);
    row.layout.justify_content = Some(taffy::JustifyContent::END);
    let buttons = ui.add(id, "buttons", row, super::Container::group())?;
    let (cid, cancel_id) = (spec.id.clone(), spec.id.clone());
    let cancel = ui.add(
        buttons,
        "cancel",
        NodeStyle::leaf(),
        Button::new(spec.cancel.as_str()).on_press(move |cx| {
            cx.action(DialogResult {
                dialog: cancel_id.clone(),
                confirmed: false,
            });
            cx.close_own_popup();
        }),
    )?;
    let confirm = ui.add(
        buttons,
        "confirm",
        NodeStyle::leaf(),
        Button::new(spec.confirm.as_str())
            .variant(if spec.destructive {
                Variant::Danger
            } else {
                Variant::Primary
            })
            .on_press(move |cx| {
                cx.action(DialogResult {
                    dialog: cid.clone(),
                    confirmed: true,
                });
                cx.close_own_popup();
            }),
    )?;
    // The safe default takes focus.
    ui.pending_popup_focus = None;
    ui.set_focus(Some(if spec.destructive { cancel } else { confirm }), true);
    Ok(id)
}

/// A button that opens a popover with a text body (and room for more: the popover is a
/// container, so `on_open` can add widgets to it).
pub struct PopoverButton {
    label: Bind<String>,
    body: String,
    size: Size,
    open: bool,
}

impl PopoverButton {
    pub fn new(label: impl Into<Bind<String>>, body: &str) -> Self {
        Self {
            label: label.into(),
            body: body.to_string(),
            size: Size::new(260.0, 120.0),
            open: false,
        }
    }
    pub fn size(mut self, s: Size) -> Self {
        self.size = s;
        self
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// A popover's panel.
pub struct PopoverPanel {
    body: String,
}

impl Widget for PopoverPanel {
    fn role(&self) -> Role {
        Role::Dialog
    }
    fn focusable(&self) -> bool {
        true
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let shadow = cx.theme().shadows[2];
        cx.panel(
            r,
            Some(ColorRole::BgRaised),
            Some(ColorRole::Border),
            cx.theme().radius.md,
            Some(shadow),
        );
        let mut style = body(cx.theme());
        style.wrap = true;
        let pad = cx.theme().space[3];
        cx.text(
            &self.body,
            &style,
            Point::new(r.x + pad, r.y + pad),
            ColorRole::FgPrimary,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.body.as_str());
    }
}

impl Widget for PopoverButton {
    fn role(&self) -> Role {
        Role::Button
    }
    fn focusable(&self) -> bool {
        true
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        cx: &mut MeasureCx,
        _k: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let (_, t) = cx
            .text
            .layout(&self.label.get(cx.rt), &body(cx.theme), None);
        Size::new(
            t.w + 2.0 * control_pad_x(cx.theme) + 14.0,
            super::CONTROL_MIN_H,
        )
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if super::activation(ev) {
            if !self.open {
                let r = cx.rect();
                let panel = PopoverPanel {
                    body: self.body.clone(),
                };
                if cx
                    .open_popup(
                        Key::Static("popover"),
                        PopupSpec::popover(Anchor::Below(r), self.size),
                        panel,
                    )
                    .is_some()
                {
                    self.open = true;
                    cx.request_paint();
                    cx.request_a11y();
                }
            }
            return Handled::Yes;
        }
        if let UiEvent::PopupClosed(_) = ev {
            self.open = false;
            cx.request_paint();
            cx.request_a11y();
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let st = button_style_for(Variant::Secondary, cx.state());
        cx.panel(r, st.bg, st.border, cx.theme().radius.md, None);
        let text = self.label.get(cx.rt());
        let style = body(cx.theme());
        let (_, t) = cx.shape(&text, &style);
        let pad = control_pad_x(cx.theme());
        cx.text(
            &text,
            &style,
            Point::new(r.x + pad, r.y + (r.h - t.h) * 0.5),
            st.fg,
        );
        cx.text(
            "▾",
            &style,
            Point::new(r.right() - pad - 4.0, r.y + (r.h - t.h) * 0.5),
            st.fg,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.get(cx.rt));
        node.set_expanded(self.open);
        node.set_has_popup(accesskit::HasPopup::Dialog);
        node.add_action(accesskit::Action::Click);
    }
}
