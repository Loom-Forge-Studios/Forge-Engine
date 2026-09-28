//! Curve and gradient editors (§21.16 "Editors"), drawn through the mesh pipeline
//! (`lyon` strokes and per-vertex-coloured grids, §21.13).
//!
//! Both are one tab stop holding a list of keys (AccessKit list box; each key is a virtual
//! option announcing its values), and both are fully keyboard-operable:
//!
//! | Key | Curve editor | Gradient editor |
//! |---|---|---|
//! | Ctrl+Left / Ctrl+Right, Home / End | previous / next, first / last key | same, for stops |
//! | Left / Right (Shift: fine) | move the key in time | move the stop |
//! | Up / Down (Shift: fine) | change the key's value | — |
//! | Insert | add a key between this one and the next | add a stop (colour interpolated) |
//! | Delete | remove the key (one always stays) | remove the stop (two always stay) |
//! | Enter | cycle the segment's interpolation | edit the stop's colour (popover picker) |
//!
//! The pointer drags keys and stops; a double click adds one. Edits write the model
//! signal live (a preview) and raise [`CurveEdited`] / [`GradientEdited`] once per
//! gesture, which the editor turns into one undoable command (§21.18).
//! An unchanged curve re-registers the same mesh id, so repainting tessellates nothing.

use accesskit::{Action, Role};

use crate::color::{HdrColor, sample_stops};
use crate::damage::Dirty;
use crate::geom::{Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::render::MeshData;
use crate::render::mesh::PathBuilder;
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::small;

const HIT: f32 = 7.0;
const PAD: f32 = 10.0;

// ---- curves ------------------------------------------------------------------------------

/// How a segment interpolates from its key to the next.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Interp {
    Constant,
    Linear,
    /// Cubic Hermite with Catmull-Rom tangents.
    #[default]
    Smooth,
}

impl Interp {
    fn next(self) -> Self {
        match self {
            Interp::Smooth => Interp::Linear,
            Interp::Linear => Interp::Constant,
            Interp::Constant => Interp::Smooth,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Interp::Constant => "constant",
            Interp::Linear => "linear",
            Interp::Smooth => "smooth",
        }
    }
}

/// A key: time, value, and how the segment after it interpolates.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct CurveKey {
    pub t: f64,
    pub v: f64,
    pub interp: Interp,
}

/// A 1-D curve: keys sorted by time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve {
    pub keys: Vec<CurveKey>,
}

impl Curve {
    pub fn new(mut keys: Vec<CurveKey>) -> Self {
        keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        Self { keys }
    }
    /// Evaluate at `t` (clamped to the first/last key outside the keyed range).
    pub fn eval(&self, t: f64) -> f64 {
        let k = &self.keys;
        match k.len() {
            0 => 0.0,
            1 => k[0].v,
            n => {
                if t <= k[0].t {
                    return k[0].v;
                }
                if t >= k[n - 1].t {
                    return k[n - 1].v;
                }
                let i = k.partition_point(|x| x.t <= t).saturating_sub(1).min(n - 2);
                let (a, b) = (k[i], k[i + 1]);
                let span = (b.t - a.t).max(f64::EPSILON);
                let u = ((t - a.t) / span).clamp(0.0, 1.0);
                match a.interp {
                    Interp::Constant => a.v,
                    Interp::Linear => a.v + (b.v - a.v) * u,
                    Interp::Smooth => {
                        let slope = |j: usize| -> f64 {
                            let p = k[j.saturating_sub(1)];
                            let q = k[(j + 1).min(n - 1)];
                            if (q.t - p.t).abs() < f64::EPSILON {
                                0.0
                            } else {
                                (q.v - p.v) / (q.t - p.t)
                            }
                        };
                        let (m0, m1) = (slope(i) * span, slope(i + 1) * span);
                        let (u2, u3) = (u * u, u * u * u);
                        (2.0 * u3 - 3.0 * u2 + 1.0) * a.v
                            + (u3 - 2.0 * u2 + u) * m0
                            + (-2.0 * u3 + 3.0 * u2) * b.v
                            + (u3 - u2) * m1
                    }
                }
            }
        }
    }
}

/// Raised once per edit gesture (a drag's release, each keyboard edit).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CurveEdited {
    pub editor: WidgetId,
}

/// The curve editor over a fixed view range.
pub struct CurveEditor {
    value: Signal<Curve>,
    label: String,
    t_range: (f64, f64),
    v_range: (f64, f64),
    selected: usize,
    drag: bool,
    /// Fault (W2 control for the catalogue idle check): request an animation frame on
    /// every event, so the editor never lets the UI idle.
    never_idle: crate::controls::Switch,
}

impl CurveEditor {
    pub fn new(value: Signal<Curve>, label: &str) -> Self {
        Self {
            value,
            label: label.to_string(),
            t_range: (0.0, 1.0),
            v_range: (0.0, 1.0),
            selected: 0,
            drag: false,
            never_idle: crate::controls::Switch::default(),
        }
    }
    #[cfg(any(test, feature = "controls"))]
    #[doc(hidden)]
    pub fn with_fault_never_idle(mut self) -> Self {
        self.never_idle.on = true;
        self
    }
    pub fn range(mut self, t: (f64, f64), v: (f64, f64)) -> Self {
        if t.1 > t.0 {
            self.t_range = t;
        }
        if v.1 > v.0 {
            self.v_range = v;
        }
        self
    }
    pub fn selected(&self) -> usize {
        self.selected
    }
    /// Change the view range of a built editor (the sequencer fits it to the track shown);
    /// an empty range on either axis leaves that axis as it was. The caller repaints.
    pub fn set_range(&mut self, t: (f64, f64), v: (f64, f64)) {
        if t.1 > t.0 {
            self.t_range = t;
        }
        if v.1 > v.0 {
            self.v_range = v;
        }
        self.selected = 0;
    }
    /// The model signal the editor writes.
    pub fn model(&self) -> Signal<Curve> {
        self.value
    }
    /// The view range: `(time, value)`.
    pub fn view_range(&self) -> ((f64, f64), (f64, f64)) {
        (self.t_range, self.v_range)
    }
    fn plot(&self, r: Rect) -> Rect {
        Rect::new(
            r.x + PAD,
            r.y + PAD,
            (r.w - 2.0 * PAD).max(1.0),
            (r.h - 2.0 * PAD).max(1.0),
        )
    }
    fn to_px(&self, p: Rect, t: f64, v: f64) -> Point {
        let u = (t - self.t_range.0) / (self.t_range.1 - self.t_range.0);
        let w = (v - self.v_range.0) / (self.v_range.1 - self.v_range.0);
        Point::new(p.x + u as f32 * p.w, p.bottom() - w as f32 * p.h)
    }
    fn value_at_px(&self, p: Rect, pt: Point) -> (f64, f64) {
        let u = f64::from(((pt.x - p.x) / p.w).clamp(0.0, 1.0));
        let w = f64::from(((p.bottom() - pt.y) / p.h).clamp(0.0, 1.0));
        (
            self.t_range.0 + u * (self.t_range.1 - self.t_range.0),
            self.v_range.0 + w * (self.v_range.1 - self.v_range.0),
        )
    }
    /// Move key `i` to `(t, v)`, kept between its neighbours in time.
    fn place(c: &mut Curve, i: usize, t: f64, v: f64) {
        let lo = if i > 0 {
            c.keys[i - 1].t + 1e-6
        } else {
            f64::MIN
        };
        let hi = c.keys.get(i + 1).map_or(f64::MAX, |k| k.t - 1e-6);
        if let Some(k) = c.keys.get_mut(i) {
            k.t = t.clamp(lo, hi.max(lo));
            k.v = v;
        }
    }
    fn edited(&self, cx: &mut EventCx) {
        let id = cx.id();
        cx.action(CurveEdited { editor: id });
        cx.request_a11y();
    }
}

impl Widget for CurveEditor {
    fn role(&self) -> Role {
        Role::ListBox
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
        Size::new(known.width.unwrap_or(320.0), known.height.unwrap_or(160.0))
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if self.never_idle.on() {
            cx.request_anim_frame();
        }
        let mut c = self.value.get(cx.rt());
        let n = c.keys.len();
        if n > 0 {
            self.selected = self.selected.min(n - 1);
        }
        let dt = (self.t_range.1 - self.t_range.0) * 0.01;
        let dv = (self.v_range.1 - self.v_range.0) * 0.01;
        match ev {
            UiEvent::Key(k) if k.pressed => {
                let fine = if k.mods.shift { 0.1 } else { 1.0 };
                let i = self.selected;
                match k.code {
                    KeyCode::Left if k.mods.ctrl => self.selected = i.saturating_sub(1),
                    KeyCode::Right if k.mods.ctrl => {
                        self.selected = (i + 1).min(n.saturating_sub(1))
                    }
                    KeyCode::Home => self.selected = 0,
                    KeyCode::End => self.selected = n.saturating_sub(1),
                    KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if n > 0 => {
                        let key = c.keys[i];
                        let (t, v) = match k.code {
                            KeyCode::Left => (key.t - dt * fine, key.v),
                            KeyCode::Right => (key.t + dt * fine, key.v),
                            KeyCode::Up => (key.t, key.v + dv * fine),
                            _ => (key.t, key.v - dv * fine),
                        };
                        Self::place(&mut c, i, t, v);
                        self.value.set(cx.rt_mut(), c);
                        self.edited(cx);
                    }
                    KeyCode::Insert => {
                        let t = match (c.keys.get(i), c.keys.get(i + 1)) {
                            (Some(a), Some(b)) => (a.t + b.t) * 0.5,
                            (Some(a), None) => a.t + dt * 10.0,
                            _ => self.t_range.0,
                        };
                        let key = CurveKey {
                            t,
                            v: c.eval(t),
                            interp: c.keys.get(i).map_or(Interp::Smooth, |k| k.interp),
                        };
                        let at = c.keys.partition_point(|k| k.t <= t);
                        c.keys.insert(at, key);
                        self.selected = at;
                        self.value.set(cx.rt_mut(), c);
                        self.edited(cx);
                    }
                    KeyCode::Delete | KeyCode::Backspace if n > 1 => {
                        c.keys.remove(i);
                        self.selected = i.min(n - 2);
                        self.value.set(cx.rt_mut(), c);
                        self.edited(cx);
                    }
                    KeyCode::Enter if n > 0 => {
                        c.keys[i].interp = c.keys[i].interp.next();
                        self.value.set(cx.rt_mut(), c);
                        self.edited(cx);
                    }
                    _ => return Handled::No,
                }
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
            } => {
                let p = self.plot(cx.rect());
                let hit = c.keys.iter().position(|k| {
                    let q = self.to_px(p, k.t, k.v);
                    (q.x - pos.x).abs() <= HIT && (q.y - pos.y).abs() <= HIT
                });
                match hit {
                    Some(i) => {
                        self.selected = i;
                        self.drag = true;
                        cx.capture_pointer();
                    }
                    None if *clicks >= 2 => {
                        let (t, v) = self.value_at_px(p, *pos);
                        let at = c.keys.partition_point(|k| k.t <= t);
                        c.keys.insert(
                            at,
                            CurveKey {
                                t,
                                v,
                                interp: Interp::Smooth,
                            },
                        );
                        self.selected = at;
                        self.value.set(cx.rt_mut(), c);
                        self.edited(cx);
                    }
                    None => {}
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } if self.drag => {
                let p = self.plot(cx.rect());
                let (t, v) = self.value_at_px(p, *pos);
                Self::place(&mut c, self.selected, t, v);
                self.value.set(cx.rt_mut(), c);
                Handled::Yes
            }
            UiEvent::PointerUp { .. } if self.drag => {
                self.drag = false;
                cx.release_pointer();
                self.edited(cx);
                Handled::Yes
            }
            UiEvent::A11yChildAction(i, Action::Click | Action::Focus) => {
                self.selected = (*i as usize).min(n.saturating_sub(1));
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let rad = cx.theme().radius.sm;
        cx.panel(
            r,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            rad,
            None,
        );
        let p = self.plot(r);
        // Grid: quarters.
        for i in 1..4 {
            let f = i as f32 / 4.0;
            cx.mark_alpha(
                Rect::new(p.x + p.w * f, p.y, 1.0, p.h),
                ColorRole::Border,
                ColorRole::BgSunken,
                0.0,
                0.5,
            );
            cx.mark_alpha(
                Rect::new(p.x, p.y + p.h * f, p.w, 1.0),
                ColorRole::Border,
                ColorRole::BgSunken,
                0.0,
                0.5,
            );
        }
        let c = self.value.get(cx.rt());
        if !c.keys.is_empty() {
            let mut path = PathBuilder::new();
            let samples = (p.w as usize / 3).clamp(16, 400);
            for s in 0..=samples {
                let t =
                    self.t_range.0 + (self.t_range.1 - self.t_range.0) * s as f64 / samples as f64;
                let q = self.to_px(
                    p,
                    t,
                    c.eval(t).clamp(self.v_range.0 - 1e3, self.v_range.1 + 1e3),
                );
                let q = Point::new(q.x, q.y.clamp(r.y, r.bottom()));
                if s == 0 {
                    path.move_to(q);
                } else {
                    path.line_to(q);
                }
            }
            cx.stroke_path(&path.build(), 2.0, ColorRole::Accent, ColorRole::BgSunken);
        }
        let focused = cx.state().focused;
        for (i, k) in c.keys.iter().enumerate() {
            let q = self.to_px(p, k.t, k.v);
            let sel = i == self.selected;
            let d = if sel { 11.0 } else { 8.0 };
            let kr = Rect::new(q.x - d * 0.5, q.y - d * 0.5, d, d);
            if sel && focused {
                cx.panel(kr.outset(3.0), None, Some(ColorRole::FocusRing), 3.0, None);
            }
            cx.mark(
                kr,
                if sel {
                    ColorRole::Accent
                } else {
                    ColorRole::FgPrimary
                },
                ColorRole::BgSunken,
                2.0,
            );
        }
        if let Some(k) = c.keys.get(self.selected) {
            let st = small(cx.theme());
            let text = format!("t {:.3}  v {:.3}  {}", k.t, k.v, k.interp.name());
            cx.text(
                &text,
                &st,
                Point::new(r.x + 6.0, r.y + 2.0),
                ColorRole::FgMuted,
            );
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_description(crate::tr!("Ctrl+arrows select a key; arrows move it; Insert adds; Delete removes; Enter changes interpolation"));
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let c = self.value.get(cx.rt);
        let n = c.keys.len();
        for (i, k) in c.keys.iter().enumerate() {
            let mut node = accesskit::Node::new(Role::ListBoxOption);
            node.set_label(crate::trf!(
                "Key {index}: time {t}, value {v}, {interp}",
                index = i + 1,
                t = format!("{:.3}", k.t),
                v = format!("{:.3}", k.v),
                interp = k.interp.name()
            ));
            node.set_selected(i == self.selected);
            node.set_position_in_set(i + 1);
            node.set_size_of_set(n);
            node.add_action(Action::Click);
            out.push((i as u64, node));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        Some(self.selected as u64)
    }
}

// ---- gradients ---------------------------------------------------------------------------

/// A colour stop.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct GradientStop {
    /// 0..=1 along the gradient.
    pub pos: f32,
    pub color: HdrColor,
}

/// A colour gradient: stops sorted by position (≥ 2).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gradient {
    pub stops: Vec<GradientStop>,
}

impl Gradient {
    pub fn new(mut stops: Vec<GradientStop>) -> Self {
        stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
        Self { stops }
    }
    /// The colour at `u` (linear light; interpolated per channel).
    pub fn eval(&self, u: f32) -> HdrColor {
        let s = &self.stops;
        match s.len() {
            0 => HdrColor::default(),
            1 => s[0].color,
            n => {
                if u <= s[0].pos {
                    return s[0].color;
                }
                if u >= s[n - 1].pos {
                    return s[n - 1].color;
                }
                let i = s
                    .partition_point(|x| x.pos <= u)
                    .saturating_sub(1)
                    .min(n - 2);
                let (a, b) = (s[i], s[i + 1]);
                let t = ((u - a.pos) / (b.pos - a.pos).max(f32::EPSILON)).clamp(0.0, 1.0);
                let l = |x: f32, y: f32| x + (y - x) * t;
                HdrColor::linear(
                    l(a.color.r, b.color.r),
                    l(a.color.g, b.color.g),
                    l(a.color.b, b.color.b),
                    l(a.color.a, b.color.a),
                )
            }
        }
    }
}

/// Raised once per gradient edit gesture.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GradientEdited {
    pub editor: WidgetId,
}

const BAR_H: f32 = 22.0;
const HANDLE: f32 = 12.0;

/// The gradient editor.
pub struct GradientEditor {
    value: Signal<Gradient>,
    /// The selected stop's colour, edited by the popover picker (bound at insertion).
    stop_color: Signal<HdrColor>,
    label: String,
    selected: usize,
    drag: bool,
    popup: Option<WidgetId>,
}

impl GradientEditor {
    /// `stop_color` is a scratch signal the editor owns (create it with
    /// `rt.signal(HdrColor::default())`); the popover picker edits the selected stop
    /// through it.
    pub fn new(value: Signal<Gradient>, stop_color: Signal<HdrColor>, label: &str) -> Self {
        Self {
            value,
            stop_color,
            label: label.to_string(),
            selected: 0,
            drag: false,
            popup: None,
        }
    }
    pub fn selected(&self) -> usize {
        self.selected
    }
    pub fn is_picking(&self) -> bool {
        self.popup.is_some()
    }
    fn bar(&self, r: Rect) -> Rect {
        Rect::new(
            r.x + HANDLE,
            r.y + 2.0,
            (r.w - 2.0 * HANDLE).max(1.0),
            BAR_H,
        )
    }
    fn select(&mut self, cx: &mut EventCx, g: &Gradient, i: usize) {
        self.selected = i.min(g.stops.len().saturating_sub(1));
        if let Some(s) = g.stops.get(self.selected) {
            self.stop_color.set(cx.rt_mut(), s.color);
        }
        cx.request_paint();
        cx.request_a11y();
    }
    fn edited(&self, cx: &mut EventCx) {
        let id = cx.id();
        cx.action(GradientEdited { editor: id });
        cx.request_a11y();
    }
    fn move_stop(g: &mut Gradient, i: usize, pos: f32) -> usize {
        let Some(mut s) = g.stops.get(i).copied() else {
            return i;
        };
        s.pos = pos.clamp(0.0, 1.0);
        g.stops.remove(i);
        let at = g.stops.partition_point(|x| x.pos <= s.pos);
        g.stops.insert(at, s);
        at
    }
}

impl Widget for GradientEditor {
    fn role(&self) -> Role {
        Role::ListBox
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.value.any()), Dirty::PAINT | Dirty::A11Y);
        b.watch(Some(self.stop_color.any()), Dirty::NONE);
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
        Size::new(known.width.unwrap_or(320.0), BAR_H + HANDLE + 22.0)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let mut g = self.value.get(cx.rt());
        let n = g.stops.len();
        match ev {
            UiEvent::BindingChanged => {
                // The popover picker changed the selected stop's colour.
                let c = self.stop_color.get(cx.rt());
                if let Some(s) = g.stops.get_mut(self.selected)
                    && self.popup.is_some()
                    && s.color != c
                {
                    s.color = c;
                    self.value.set(cx.rt_mut(), g);
                }
                Handled::Yes
            }
            UiEvent::PopupClosed(p) if Some(*p) == self.popup => {
                self.popup = None;
                self.edited(cx);
                Handled::Yes
            }
            UiEvent::FocusGained { .. } => {
                let i = self.selected;
                self.select(cx, &g, i);
                Handled::No
            }
            UiEvent::Key(k) if k.pressed => {
                let i = self.selected.min(n.saturating_sub(1));
                let step = if k.mods.shift { 0.001 } else { 0.01 };
                match k.code {
                    KeyCode::Left if k.mods.ctrl => self.select(cx, &g, i.saturating_sub(1)),
                    KeyCode::Right if k.mods.ctrl => self.select(cx, &g, i + 1),
                    KeyCode::Home => self.select(cx, &g, 0),
                    KeyCode::End => self.select(cx, &g, n.saturating_sub(1)),
                    KeyCode::Left | KeyCode::Right if n > 0 => {
                        let d = if k.code == KeyCode::Left { -step } else { step };
                        let pos = g.stops[i].pos + d;
                        self.selected = Self::move_stop(&mut g, i, pos);
                        self.value.set(cx.rt_mut(), g);
                        self.edited(cx);
                    }
                    KeyCode::Insert => {
                        let pos = match (g.stops.get(i), g.stops.get(i + 1)) {
                            (Some(a), Some(b)) => (a.pos + b.pos) * 0.5,
                            (Some(a), None) => (a.pos + 1.0) * 0.5,
                            _ => 0.5,
                        };
                        let color = g.eval(pos);
                        let at = g.stops.partition_point(|x| x.pos <= pos);
                        g.stops.insert(at, GradientStop { pos, color });
                        self.value.set(cx.rt_mut(), g.clone());
                        self.select(cx, &g, at);
                        self.edited(cx);
                    }
                    KeyCode::Delete | KeyCode::Backspace if n > 2 => {
                        g.stops.remove(i);
                        self.value.set(cx.rt_mut(), g.clone());
                        self.select(cx, &g, i.min(n - 2));
                        self.edited(cx);
                    }
                    KeyCode::Enter | KeyCode::Space if n > 0 => {
                        self.select(cx, &g, i);
                        let r = cx.rect();
                        let bar = self.bar(r);
                        let x = bar.x + g.stops[i].pos * bar.w;
                        let anchor = Rect::new(x - HANDLE * 0.5, bar.bottom(), HANDLE, HANDLE);
                        self.popup = super::open_color_popover(
                            cx,
                            anchor,
                            self.stop_color,
                            &crate::trf!("{label} stop {n}", label = self.label, n = i + 1),
                        );
                    }
                    _ => return Handled::No,
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
            } => {
                let bar = self.bar(cx.rect());
                let hit = g.stops.iter().position(|s| {
                    let x = bar.x + s.pos * bar.w;
                    (pos.x - x).abs() <= HANDLE * 0.75
                        && pos.y >= bar.y
                        && pos.y <= bar.bottom() + HANDLE + 2.0
                });
                match hit {
                    Some(i) => {
                        self.select(cx, &g, i);
                        self.drag = true;
                        cx.capture_pointer();
                    }
                    None if *clicks >= 2 => {
                        let u = ((pos.x - bar.x) / bar.w).clamp(0.0, 1.0);
                        let color = g.eval(u);
                        let at = g.stops.partition_point(|x| x.pos <= u);
                        g.stops.insert(at, GradientStop { pos: u, color });
                        self.value.set(cx.rt_mut(), g.clone());
                        self.select(cx, &g, at);
                        self.edited(cx);
                    }
                    None => {}
                }
                Handled::Yes
            }
            UiEvent::PointerMove { pos } if self.drag => {
                let bar = self.bar(cx.rect());
                let u = (pos.x - bar.x) / bar.w;
                self.selected = Self::move_stop(&mut g, self.selected, u);
                self.value.set(cx.rt_mut(), g);
                Handled::Yes
            }
            UiEvent::PointerUp { .. } if self.drag => {
                self.drag = false;
                cx.release_pointer();
                self.edited(cx);
                Handled::Yes
            }
            UiEvent::A11yChildAction(i, Action::Click | Action::Focus) => {
                let i = *i as usize;
                self.select(cx, &g, i);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let g = self.value.get(cx.rt());
        let bar = self.bar(r);
        let rad = cx.theme().radius.sm;
        let stops: Vec<(f32, crate::geom::Color)> =
            g.stops.iter().map(|s| (s.pos, s.color.display())).collect();
        let mut m = MeshData::default();
        m.color_grid(bar, 64, 1, |u, _| sample_stops(&stops, u));
        cx.mesh(m);
        cx.panel(bar, None, Some(ColorRole::Border), rad, None);
        let bg = cx.parent_bg();
        let focused = cx.state().focused;
        for (i, s) in g.stops.iter().enumerate() {
            let x = bar.x + s.pos * bar.w;
            let hr = Rect::new(x - HANDLE * 0.5, bar.bottom() + 2.0, HANDLE, HANDLE);
            let sel = i == self.selected;
            if sel && focused {
                cx.panel(hr.outset(3.0), None, Some(ColorRole::FocusRing), 3.0, None);
            }
            cx.mark(
                hr,
                if sel {
                    ColorRole::Accent
                } else {
                    ColorRole::FgPrimary
                },
                bg,
                2.0,
            );
            let c = s.color.display_opaque();
            let mut sw = MeshData::default();
            sw.color_grid(hr.outset(-2.5), 1, 1, |_, _| c);
            cx.mesh(sw);
            cx.mark(
                Rect::new(x - 0.5, bar.y, 1.0, bar.h),
                ColorRole::FgPrimary,
                ColorRole::BgBase,
                0.0,
            );
        }
        if let Some(s) = g.stops.get(self.selected) {
            let st = small(cx.theme());
            let text = crate::trf!(
                "stop {n} at {pos} %  {hex}",
                n = self.selected + 1,
                pos = format!("{:.1}", s.pos * 100.0),
                hex = s.color.hex()
            );
            cx.text(
                &text,
                &st,
                Point::new(bar.x, bar.bottom() + HANDLE + 4.0),
                ColorRole::FgMuted,
            );
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_description(crate::tr!("Ctrl+arrows select a stop; arrows move it; Insert adds; Delete removes; Enter edits its colour"));
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let g = self.value.get(cx.rt);
        let n = g.stops.len();
        for (i, s) in g.stops.iter().enumerate() {
            let mut node = accesskit::Node::new(Role::ListBoxOption);
            node.set_label(crate::trf!(
                "Stop {index}: {first} %, {hex}",
                index = i + 1,
                first = format!("{:.1}", s.pos * 100.0),
                hex = s.color.hex()
            ));
            node.set_selected(i == self.selected);
            node.set_position_in_set(i + 1);
            node.set_size_of_set(n);
            node.add_action(Action::Click);
            out.push((i as u64, node));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        Some(self.selected as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_interpolates_each_mode() {
        let mk = |interp| {
            Curve::new(vec![
                CurveKey {
                    t: 0.0,
                    v: 0.0,
                    interp,
                },
                CurveKey {
                    t: 1.0,
                    v: 1.0,
                    interp,
                },
            ])
        };
        assert_eq!(mk(Interp::Linear).eval(0.25), 0.25);
        assert_eq!(mk(Interp::Constant).eval(0.75), 0.0);
        let s = mk(Interp::Smooth);
        assert!((s.eval(0.5) - 0.5).abs() < 1e-12, "symmetric");
        assert_eq!(s.eval(-1.0), 0.0);
        assert_eq!(s.eval(2.0), 1.0);
    }

    #[test]
    fn gradient_interpolates_in_linear_light() {
        let g = Gradient::new(vec![
            GradientStop {
                pos: 1.0,
                color: HdrColor::linear(0.0, 0.0, 2.0, 1.0),
            },
            GradientStop {
                pos: 0.0,
                color: HdrColor::linear(1.0, 0.0, 0.0, 1.0),
            },
        ]);
        assert_eq!(g.stops[0].pos, 0.0, "sorted");
        let mid = g.eval(0.5);
        assert!(
            (mid.r - 0.5).abs() < 1e-6 && (mid.b - 1.0).abs() < 1e-6,
            "{mid:?}"
        );
    }
}
