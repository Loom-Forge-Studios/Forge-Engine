//! The colour picker (§21.16: "HDR-aware: linear/sRGB, exposure, alpha").
//!
//! A composite ([`color_picker`]) of real widgets, so every part is its own tab stop and
//! its own AccessKit node:
//!
//! * a **saturation/value** surface (Left/Right: saturation, Up/Down: value; Shift fine,
//!   PageUp/PageDown coarse; drag with the pointer);
//! * a **hue** strip and an **alpha** strip (sliders; arrows, PageUp/PageDown, Home/End);
//! * an **exposure** field in stops (EV ≥ 0): the colour is the displayable base colour
//!   times `2^EV`, so an emissive `4.0` is "white at +2 EV", never clipped in the model;
//! * a **Linear / sRGB** switch for how channels are shown and typed;
//! * a **hex** entry (sRGB, for designers) and a swatch with the channel readout.
//!
//! The model is a `Signal<HdrColor>` (linear light). The parts share a `Signal<Hsve>` so
//! a grey keeps its hue while you drag saturation back up; the picker's root reconciles
//! an outside change of the colour (undo, another view) into the parts.
//!
//! [`ColorButton`] is the compact form (a swatch that opens the picker in a popover) for
//! property grids and the gradient editor.

use accesskit::{Action, Role};

use crate::UiError;
use crate::color::{ColorField, ColorSpaceMode, HdrColor, Hsve};
use crate::damage::Dirty;
use crate::geom::{Point, Rect, Size};
use crate::id::{Key, WidgetId};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::layout::NodeStyle;
use crate::overlay::{Anchor, PopupSpec};
use crate::render::MeshData;
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::line_edit::{EditOutcome, LineEdit};
use super::{Build, CONTROL_MIN_H, NumericField, SegmentedControl, body, small};

/// The ids a colour picker is made of.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColorPickerParts {
    pub picker: WidgetId,
    pub sat_val: WidgetId,
    pub hue: WidgetId,
    pub alpha: WidgetId,
    pub exposure: WidgetId,
    pub mode: WidgetId,
    pub hex: WidgetId,
    pub swatch: WidgetId,
}

/// Build a colour picker for `value` under `parent`.
pub fn color_picker(
    b: &mut dyn Build,
    parent: WidgetId,
    key: impl Into<Key>,
    value: Signal<HdrColor>,
    label: &str,
) -> Result<ColorPickerParts, UiError> {
    let space = b.theme_ref().space;
    let init = value.get(b.runtime());
    let h0 = Hsve::from_color(init, None);
    let hsve = b.signal(h0);
    let ev = b.signal(f64::from(h0.ev));
    let mode = b.signal(0usize);
    let picker = b.add(
        parent,
        key,
        NodeStyle::column(space[2]),
        ColorPicker {
            label: label.to_string(),
            value,
            hsve,
            ev,
        },
    )?;
    let sat_val = b.add(
        picker,
        "sv",
        NodeStyle::leaf().size(220.0, 140.0),
        ColorSurface::new(Surface::SatVal, value, hsve, label),
    )?;
    let hue = b.add(
        picker,
        "hue",
        NodeStyle::leaf().size(220.0, 18.0),
        ColorSurface::new(Surface::Hue, value, hsve, label),
    )?;
    let alpha = b.add(
        picker,
        "alpha",
        NodeStyle::leaf().size(220.0, 18.0),
        ColorSurface::new(Surface::Alpha, value, hsve, label),
    )?;
    let row = b.add(
        picker,
        "row",
        NodeStyle::row(space[2]),
        super::Container::group(),
    )?;
    let exposure = b.add(
        row,
        "exposure",
        NodeStyle::leaf().width(110.0),
        NumericField::new(ev, crate::tr!("Exposure (EV)"))
            .range(0.0, 24.0)
            .step(0.1)
            .decimals(2),
    )?;
    let mode_id = b.add(
        row,
        "mode",
        NodeStyle::leaf(),
        SegmentedControl::new(
            crate::tr!("Channel space"),
            &[crate::tr!("Linear"), crate::tr!("sRGB")],
            mode,
        ),
    )?;
    let row2 = b.add(
        picker,
        "row2",
        NodeStyle::row(space[2]),
        super::Container::group(),
    )?;
    let swatch = b.add(
        row2,
        "swatch",
        NodeStyle::leaf(),
        ColorSwatch {
            value,
            mode,
            original: init,
        },
    )?;
    let hex = b.add(
        row2,
        "hex",
        NodeStyle::leaf().width(100.0),
        HexField::new(value),
    )?;
    Ok(ColorPickerParts {
        picker,
        sat_val,
        hue,
        alpha,
        exposure,
        mode: mode_id,
        hex,
        swatch,
    })
}

/// The picker's root: reconciles the colour, the shared hue/sat/val and the exposure.
pub struct ColorPicker {
    label: String,
    value: Signal<HdrColor>,
    hsve: Signal<Hsve>,
    ev: Signal<f64>,
}

impl Widget for ColorPicker {
    fn role(&self) -> Role {
        Role::Group
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.value.any()), Dirty::NONE);
        b.watch(Some(self.ev.any()), Dirty::NONE);
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if *ev != UiEvent::BindingChanged {
            return Handled::No;
        }
        let v = self.value.get(cx.rt());
        let h = self.hsve.get(cx.rt());
        let e = self.ev.get(cx.rt()) as f32;
        if h.to_color() != v {
            // Changed from outside (undo, hex entry, another view): re-derive the parts.
            let n = Hsve::from_color(v, Some(h));
            self.hsve.set(cx.rt_mut(), n);
            self.ev.set(cx.rt_mut(), f64::from(n.ev));
        } else if (e - h.ev).abs() > 1e-6 {
            let n = Hsve {
                ev: e.max(0.0),
                ..h
            };
            self.hsve.set(cx.rt_mut(), n);
            self.value.set(cx.rt_mut(), n.to_color());
        }
        Handled::Yes
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
    }
}

/// Which surface.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Surface {
    SatVal,
    Hue,
    Alpha,
}

/// A picker surface (saturation/value plane, hue strip, alpha strip).
pub struct ColorSurface {
    kind: Surface,
    value: Signal<HdrColor>,
    hsve: Signal<Hsve>,
    label: String,
    dragging: bool,
}

impl ColorSurface {
    pub fn new(kind: Surface, value: Signal<HdrColor>, hsve: Signal<Hsve>, label: &str) -> Self {
        Self {
            kind,
            value,
            hsve,
            label: label.to_string(),
            dragging: false,
        }
    }
    fn write(&self, cx: &mut EventCx, h: Hsve) {
        let h = Hsve {
            h: h.h.rem_euclid(1.0),
            s: h.s.clamp(0.0, 1.0),
            v: h.v.clamp(0.0, 1.0),
            a: h.a.clamp(0.0, 1.0),
            ..h
        };
        self.hsve.set(cx.rt_mut(), h);
        self.value.set(cx.rt_mut(), h.to_color());
    }
    fn at(&self, r: Rect, p: Point, h: Hsve) -> Hsve {
        let u = ((p.x - r.x) / r.w.max(1.0)).clamp(0.0, 1.0);
        let v = ((p.y - r.y) / r.h.max(1.0)).clamp(0.0, 1.0);
        match self.kind {
            Surface::SatVal => Hsve {
                s: u,
                v: 1.0 - v,
                ..h
            },
            Surface::Hue => Hsve {
                h: u.min(0.9999),
                ..h
            },
            Surface::Alpha => Hsve { a: u, ..h },
        }
    }
    fn name(&self) -> String {
        match self.kind {
            Surface::SatVal => crate::trf!("{label} saturation and value", label = self.label),
            Surface::Hue => crate::trf!("{label} hue", label = self.label),
            Surface::Alpha => crate::trf!("{label} alpha", label = self.label),
        }
    }
}

impl Widget for ColorSurface {
    fn role(&self) -> Role {
        Role::Slider
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.hsve.any()), Dirty::PAINT | Dirty::A11Y);
    }
    fn focusable(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let h = self.hsve.get(cx.rt());
        match ev {
            UiEvent::Key(k) if k.pressed => {
                let step = if k.mods.shift { 0.005 } else { 0.02 };
                let big = 0.1;
                let n = match (self.kind, &k.code) {
                    (Surface::SatVal, KeyCode::Right) => Hsve { s: h.s + step, ..h },
                    (Surface::SatVal, KeyCode::Left) => Hsve { s: h.s - step, ..h },
                    (Surface::SatVal, KeyCode::Up) => Hsve { v: h.v + step, ..h },
                    (Surface::SatVal, KeyCode::Down) => Hsve { v: h.v - step, ..h },
                    (Surface::SatVal, KeyCode::PageUp) => Hsve { v: h.v + big, ..h },
                    (Surface::SatVal, KeyCode::PageDown) => Hsve { v: h.v - big, ..h },
                    (Surface::SatVal, KeyCode::Home) => Hsve { s: 0.0, ..h },
                    (Surface::SatVal, KeyCode::End) => Hsve { s: 1.0, ..h },
                    (Surface::Hue, KeyCode::Right | KeyCode::Up) => Hsve {
                        h: h.h
                            + if k.mods.shift {
                                1.0 / 360.0
                            } else {
                                5.0 / 360.0
                            },
                        ..h
                    },
                    (Surface::Hue, KeyCode::Left | KeyCode::Down) => Hsve {
                        h: h.h
                            - if k.mods.shift {
                                1.0 / 360.0
                            } else {
                                5.0 / 360.0
                            },
                        ..h
                    },
                    (Surface::Hue, KeyCode::PageUp) => Hsve {
                        h: h.h + 30.0 / 360.0,
                        ..h
                    },
                    (Surface::Hue, KeyCode::PageDown) => Hsve {
                        h: h.h - 30.0 / 360.0,
                        ..h
                    },
                    (Surface::Hue, KeyCode::Home) => Hsve { h: 0.0, ..h },
                    (Surface::Alpha, KeyCode::Right | KeyCode::Up) => Hsve { a: h.a + step, ..h },
                    (Surface::Alpha, KeyCode::Left | KeyCode::Down) => Hsve { a: h.a - step, ..h },
                    (Surface::Alpha, KeyCode::PageUp) => Hsve { a: h.a + big, ..h },
                    (Surface::Alpha, KeyCode::PageDown) => Hsve { a: h.a - big, ..h },
                    (Surface::Alpha, KeyCode::Home) => Hsve { a: 0.0, ..h },
                    (Surface::Alpha, KeyCode::End) => Hsve { a: 1.0, ..h },
                    _ => return Handled::No,
                };
                self.write(cx, n);
                Handled::Yes
            }
            UiEvent::A11yAction(a @ (Action::Increment | Action::Decrement)) => {
                let d = if *a == Action::Increment { 0.02 } else { -0.02 };
                let n = match self.kind {
                    Surface::SatVal => Hsve { v: h.v + d, ..h },
                    Surface::Hue => Hsve { h: h.h + d, ..h },
                    Surface::Alpha => Hsve { a: h.a + d, ..h },
                };
                self.write(cx, n);
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                self.dragging = true;
                cx.capture_pointer();
                let n = self.at(cx.rect(), *pos, h);
                self.write(cx, n);
                Handled::Yes
            }
            UiEvent::PointerMove { pos } if self.dragging => {
                let n = self.at(cx.rect(), *pos, h);
                self.write(cx, n);
                Handled::Yes
            }
            UiEvent::PointerUp { .. } if self.dragging => {
                self.dragging = false;
                cx.release_pointer();
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let h = self.hsve.get(cx.rt());
        let bg = cx.parent_bg();
        let rad = cx.theme().radius.sm;
        if self.kind == Surface::Alpha {
            // Checkerboard under the alpha ramp, from two theme tokens.
            cx.fill(r, ColorRole::BgBase, rad);
            let s = r.h * 0.5;
            let mut i = 0;
            let mut x = r.x;
            while x < r.right() {
                let w = s.min(r.right() - x);
                let y = if i % 2 == 0 { r.y } else { r.y + s };
                cx.mark_alpha(
                    Rect::new(x, y, w, s),
                    ColorRole::FgMuted,
                    ColorRole::BgBase,
                    0.0,
                    0.35,
                );
                x += s;
                i += 1;
            }
        }
        let mut m = MeshData::default();
        let (cols, rows, field) = match self.kind {
            Surface::SatVal => (16, 16, ColorField::SatVal { h: h.h }),
            Surface::Hue => (36, 1, ColorField::Hue),
            Surface::Alpha => (
                8,
                1,
                ColorField::Alpha(
                    Hsve {
                        a: 1.0,
                        ev: 0.0,
                        ..h
                    }
                    .to_color()
                    .display(),
                ),
            ),
        };
        m.color_grid(r, cols, rows, |u, v| field.at(u, v));
        cx.mesh(m);
        cx.panel(r, None, Some(ColorRole::Border), rad, None);
        // The marker: a ring in two tokens so it reads on light and dark colours.
        let (mx, my) = match self.kind {
            Surface::SatVal => (r.x + h.s * r.w, r.y + (1.0 - h.v) * r.h),
            Surface::Hue => (r.x + h.h * r.w, r.y + r.h * 0.5),
            Surface::Alpha => (r.x + h.a * r.w, r.y + r.h * 0.5),
        };
        let d = if cx.state().focused || self.dragging {
            14.0
        } else {
            11.0
        };
        let ring = Rect::new(mx - d * 0.5, my - d * 0.5, d, d);
        cx.panel(
            ring.outset(1.0),
            None,
            Some(ColorRole::BgBase),
            d * 0.5 + 1.0,
            None,
        );
        cx.mark(
            Rect::new(ring.x, ring.y, d, d),
            ColorRole::FgPrimary,
            bg,
            d * 0.5,
        );
        cx.mark(
            ring.outset(-2.0),
            ColorRole::BgBase,
            ColorRole::FgPrimary,
            d * 0.5 - 2.0,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        let h = self.hsve.get(cx.rt);
        node.set_label(self.name());
        let (v, text) = match self.kind {
            Surface::SatVal => (
                f64::from(h.v),
                crate::trf!(
                    "saturation {s} %, value {v} %",
                    s = format!("{:.0}", h.s * 100.0),
                    v = format!("{:.0}", h.v * 100.0)
                ),
            ),
            Surface::Hue => (
                f64::from(h.h),
                crate::trf!("{n} degrees", n = format!("{:.0}", h.h * 360.0)),
            ),
            Surface::Alpha => (
                f64::from(h.a),
                crate::trf!("{n} % opaque", n = format!("{:.0}", h.a * 100.0)),
            ),
        };
        node.set_numeric_value(v);
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(1.0);
        node.set_value(text);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    }
}

/// The swatch: new colour over the original, the channels in the chosen space, and the
/// exposure when the colour is HDR.
pub struct ColorSwatch {
    value: Signal<HdrColor>,
    mode: Signal<usize>,
    original: HdrColor,
}

impl ColorSwatch {
    fn space(&self, rt: &crate::state::Runtime) -> ColorSpaceMode {
        if self.mode.get(rt) == 1 {
            ColorSpaceMode::Srgb
        } else {
            ColorSpaceMode::Linear
        }
    }
    fn readout(&self, rt: &crate::state::Runtime) -> String {
        let c = self.value.get(rt);
        let [r, g, b, a] = c.components(self.space(rt));
        let mut s = match self.space(rt) {
            ColorSpaceMode::Linear => crate::trf!(
                "linear {rgba}",
                rgba = format!("{r:.3} {g:.3} {b:.3} \u{3b1} {a:.2}")
            ),
            ColorSpaceMode::Srgb => crate::trf!(
                "sRGB {rgba}",
                rgba = format!("{r:.0} {g:.0} {b:.0} \u{3b1} {a:.0}")
            ),
        };
        if c.is_hdr() {
            s.push_str(&crate::trf!(
                "  HDR +{ev} EV",
                ev = format!("{:.2}", c.intensity().log2())
            ));
        }
        s
    }
}

impl Widget for ColorSwatch {
    fn role(&self) -> Role {
        Role::ColorWell
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.value.any()), Dirty::LAYOUT | Dirty::A11Y);
        b.watch(Some(self.mode.any()), Dirty::LAYOUT | Dirty::A11Y);
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
        let (_, t) = cx.text.layout(&self.readout(cx.rt), &small(cx.theme), None);
        Size::new(44.0 + 8.0 + t.w, CONTROL_MIN_H)
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let c = self.value.get(cx.rt());
        let sw = Rect::new(r.x, r.y + 2.0, 44.0, r.h - 4.0);
        let mut m = MeshData::default();
        let (new, old) = (c.display(), self.original.display());
        m.color_grid(Rect::new(sw.x, sw.y, sw.w, sw.h * 0.5), 1, 1, |_, _| new);
        m.color_grid(
            Rect::new(sw.x, sw.y + sw.h * 0.5, sw.w, sw.h * 0.5),
            1,
            1,
            |_, _| old,
        );
        cx.mesh(m);
        cx.panel(
            sw,
            None,
            Some(ColorRole::Border),
            cx.theme().radius.sm,
            None,
        );
        let st = small(cx.theme());
        let text = self.readout(cx.rt());
        let (_, t) = cx.shape(&text, &st);
        cx.text(
            &text,
            &st,
            Point::new(sw.right() + 8.0, r.y + (r.h - t.h) * 0.5),
            ColorRole::FgMuted,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Colour"));
        node.set_value(self.readout(cx.rt));
    }
}

/// Hex entry (sRGB). Enter or leaving the field applies; a malformed value is refused
/// with the reason shown and announced, and the colour is left unchanged.
pub struct HexField {
    value: Signal<HdrColor>,
    edit: Option<LineEdit>,
    error: Option<String>,
}

impl HexField {
    pub fn new(value: Signal<HdrColor>) -> Self {
        Self {
            value,
            edit: None,
            error: None,
        }
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    fn apply(&mut self, cx: &mut EventCx) {
        if let Some(e) = self.edit.take() {
            match HdrColor::parse_hex_input(&e.text) {
                Some(c) => {
                    // A hex value is displayable by definition (no exposure).
                    self.value.set(cx.rt_mut(), c);
                    self.error = None;
                }
                None => {
                    self.error = Some(crate::trf!(
                        "\u{201c}{text}\u{201d} is not a hex colour (#rgb, #rrggbb, #rrggbbaa)",
                        text = e.text.trim()
                    ));
                    self.edit = Some(e);
                }
            }
            cx.request_layout();
            cx.request_a11y();
        }
    }
}

impl Widget for HexField {
    fn role(&self) -> Role {
        Role::TextInput
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.value.any()), Dirty::PAINT | Dirty::A11Y);
    }
    fn focusable(&self) -> bool {
        true
    }
    fn accepts_text(&self) -> bool {
        true
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        _cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let h = if self.error.is_some() {
            CONTROL_MIN_H + 16.0
        } else {
            CONTROL_MIN_H
        };
        Size::new(known.width.unwrap_or(100.0), h)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::FocusGained { .. } => {
                let mut e = LineEdit::new(&self.value.get(cx.rt()).hex());
                e.select_all();
                self.edit = Some(e);
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::FocusLost => {
                self.apply(cx);
                self.edit = None;
                cx.request_paint();
                Handled::No
            }
            UiEvent::Key(k) if k.pressed => {
                let Some(e) = self.edit.as_mut() else {
                    return Handled::No;
                };
                match e.key(k, cx.clipboard()) {
                    EditOutcome::Submit => {
                        self.apply(cx);
                        if self.error.is_none() {
                            let mut e = LineEdit::new(&self.value.get(cx.rt()).hex());
                            e.select_all();
                            self.edit = Some(e);
                        }
                    }
                    EditOutcome::Cancel => {
                        self.edit = Some(LineEdit::new(&self.value.get(cx.rt()).hex()));
                        self.error = None;
                        cx.request_layout();
                    }
                    EditOutcome::Ignored => return Handled::No,
                    _ => {}
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::Ime(i) => {
                if let Some(e) = self.edit.as_mut() {
                    e.ime(i);
                    cx.request_paint();
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let f = Rect::new(r.x, r.y, r.w, CONTROL_MIN_H);
        let border = if self.error.is_some() {
            ColorRole::Danger
        } else {
            ColorRole::Border
        };
        cx.panel(
            f,
            Some(ColorRole::BgSunken),
            Some(border),
            cx.theme().radius.sm,
            None,
        );
        let style = body(cx.theme());
        let inner = Rect::new(f.x + 6.0, f.y, f.w - 12.0, f.h);
        match &self.edit {
            Some(e) => e.paint_line(cx, inner, &style, true, true, "rrggbb", ColorRole::BgSunken),
            None => {
                let t = self.value.get(cx.rt()).hex();
                let (run, s) = cx.shape(&t, &style);
                cx.run(
                    run,
                    Point::new(inner.x, f.y + (f.h - s.h) * 0.5),
                    ColorRole::FgPrimary,
                );
            }
        }
        if let Some(err) = &self.error {
            cx.text(
                err,
                &small(cx.theme()),
                Point::new(r.x, f.bottom() + 1.0),
                ColorRole::Danger,
            );
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Hex colour"));
        match &self.edit {
            Some(e) => node.set_value(e.text.as_str()),
            None => node.set_value(self.value.get(cx.rt).hex()),
        }
        if let Some(err) = &self.error {
            node.set_description(err.as_str());
            node.set_invalid(accesskit::Invalid::True);
        }
    }
}

/// A swatch button that opens the full picker in a popover (property grids, gradients).
pub struct ColorButton {
    value: Signal<HdrColor>,
    label: String,
    popup: Option<WidgetId>,
}

impl ColorButton {
    pub fn new(value: Signal<HdrColor>, label: &str) -> Self {
        Self {
            value,
            label: label.to_string(),
            popup: None,
        }
    }
    pub fn is_open(&self) -> bool {
        self.popup.is_some()
    }
}

/// Open a popover holding a full colour picker for `value`, anchored to `anchor`.
pub fn open_color_popover(
    cx: &mut EventCx,
    anchor: Rect,
    value: Signal<HdrColor>,
    label: &str,
) -> Option<WidgetId> {
    let host = cx.open_popup(
        Key::Static("color-popover"),
        PopupSpec::popover(Anchor::Below(anchor), Size::new(252.0, 300.0)),
        super::Container::new(Role::Dialog).labelled(label),
    )?;
    let space = cx.theme().space;
    let body_id = cx.add_widget(
        host,
        "body",
        NodeStyle::column(0.0)
            .fill()
            .padding(space[3])
            .background(ColorRole::BgRaised)
            .rounded(crate::style::Radius::Md),
        super::Container::group(),
    )?;
    color_picker(cx, body_id, "picker", value, label).ok()?;
    Some(host)
}

impl Widget for ColorButton {
    fn role(&self) -> Role {
        Role::ColorWell
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.value.any()), Dirty::PAINT | Dirty::A11Y);
    }
    fn focusable(&self) -> bool {
        true
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        _cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        Size::new(known.width.unwrap_or(56.0), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::PopupClosed(p) if Some(*p) == self.popup => {
                self.popup = None;
                Handled::Yes
            }
            e if super::activation(e) => {
                let r = cx.rect();
                self.popup = open_color_popover(cx, r, self.value, &self.label);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let c = self.value.get(cx.rt());
        let bg = if cx.state().hovered {
            ColorRole::BgHover
        } else {
            ColorRole::BgSunken
        };
        cx.panel(
            r,
            Some(bg),
            Some(ColorRole::Border),
            cx.theme().radius.sm,
            None,
        );
        // HDR is marked in words beside the swatch (never only by colour, and never on
        // top of a colour that might hide it).
        let st = small(cx.theme());
        let tag_w = if c.is_hdr() {
            cx.shape("HDR", &st).1.w + 8.0
        } else {
            0.0
        };
        let inner = r.outset(-4.0);
        let sw = Rect::new(inner.x, inner.y, (inner.w - tag_w).max(8.0), inner.h);
        let mut m = MeshData::default();
        let (d, o) = (c.display(), c.display_opaque());
        m.color_grid(Rect::new(sw.x, sw.y, sw.w * 0.5, sw.h), 1, 1, |_, _| o);
        m.color_grid(
            Rect::new(sw.x + sw.w * 0.5, sw.y, sw.w * 0.5, sw.h),
            1,
            1,
            |_, _| d,
        );
        cx.mesh(m);
        if c.is_hdr() {
            let (run, t) = cx.shape("HDR", &st);
            cx.run(
                run,
                Point::new(sw.right() + 4.0, r.y + (r.h - t.h) * 0.5),
                ColorRole::FgPrimary,
            );
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        let c = self.value.get(cx.rt);
        node.set_value(crate::trf!(
            "{hex} (linear {r}, {g}, {b}, alpha {a})",
            hex = c.hex(),
            r = format!("{:.3}", c.r),
            g = format!("{:.3}", c.g),
            b = format!("{:.3}", c.b),
            a = format!("{:.2}", c.a)
        ));
        node.add_action(Action::Click);
        node.set_has_popup(accesskit::HasPopup::Dialog);
    }
}
