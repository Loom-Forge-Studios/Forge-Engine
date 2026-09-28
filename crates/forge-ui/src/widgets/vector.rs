//! Vector, quaternion and `FramePos` editors (§21.16 "Editors").
//!
//! All three are composites of [`NumericField`]s, so each component drags, steps, takes
//! typed values with units, and is its own tab stop and AccessKit spin button. Values are
//! **`f64` end to end** — world data is never `f32` in the UI (§21.2, I1).
//!
//! * [`vector_editor`] edits `[f64; N]` (directions, scales, sizes).
//! * [`quat_editor`] edits a [`DQuat`] through an **Euler view** — yaw (about Y), pitch
//!   (about X), roll (about Z), applied yaw → pitch → roll — and shows a **gimbal
//!   warning** as pitch nears ±90°, where yaw and roll turn about the same axis and the
//!   Euler view stops being a faithful handle. The quaternion itself is shown alongside.
//! * [`frame_pos_editor`] edits a [`FramePos`]: a frame selector plus the three local
//!   components in metres. Changing the frame **re-expresses** the same point in the new
//!   frame when a rebaser is supplied (the editor's frame tree), and otherwise keeps the
//!   local offset and says so — it never silently moves an object.
//!
//! Each root reconciles its parts with its model signal: an outside change (undo,
//! another view) refreshes the fields; a field edit writes the model once.

use std::rc::Rc;

use accesskit::Role;
use forge_frames::{DQuat, DVec3, FrameId, FramePos};

use crate::UiError;
use crate::damage::Dirty;
use crate::id::{Key, WidgetId};
use crate::input::{Handled, UiEvent};
use crate::layout::NodeStyle;
use crate::state::Signal;
use crate::widget::{A11yCx, Binder, EventCx, PaintCx, Widget};

use super::{Build, ComboBox, Label, LabelKind, NumericField};

// ---- vectors -----------------------------------------------------------------------------

/// The parts of a vector editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorParts {
    pub editor: WidgetId,
    pub fields: Vec<WidgetId>,
}

const AXES: [&str; 4] = ["X", "Y", "Z", "W"];

/// Build an editor for an `N`-component vector (N ≤ 4). `unit` annotates every field.
pub fn vector_editor<const N: usize>(
    b: &mut dyn Build,
    parent: WidgetId,
    key: impl Into<Key>,
    value: Signal<[f64; N]>,
    label: &str,
    unit: Option<&str>,
) -> Result<VectorParts, UiError> {
    let space = b.theme_ref().space;
    let init = value.with(b.runtime(), |v| *v).unwrap_or([0.0; N]);
    let comps: Vec<Signal<f64>> = init.iter().map(|c| b.signal(*c)).collect();
    let editor = b.add(
        parent,
        key,
        NodeStyle::row(space[1]),
        VectorEditor::<N> {
            label: label.to_string(),
            value,
            comps: comps.clone(),
            last: init,
        },
    )?;
    let mut fields = Vec::with_capacity(N);
    for (i, c) in comps.iter().enumerate() {
        let mut f = NumericField::new(*c, &format!("{label} {}", AXES[i.min(3)])).decimals(3);
        if let Some(u) = unit {
            f = f.unit(u);
        }
        fields.push(b.add(
            editor,
            Key::Index(i as u32),
            NodeStyle::leaf().width(92.0),
            f,
        )?);
    }
    Ok(VectorParts { editor, fields })
}

/// The vector editor's root.
pub struct VectorEditor<const N: usize> {
    label: String,
    value: Signal<[f64; N]>,
    comps: Vec<Signal<f64>>,
    last: [f64; N],
}

impl<const N: usize> Widget for VectorEditor<N> {
    fn role(&self) -> Role {
        Role::Group
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.value.any()), Dirty::NONE);
        for c in &self.comps {
            b.watch(Some(c.any()), Dirty::NONE);
        }
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if *ev != UiEvent::BindingChanged {
            return Handled::No;
        }
        let v = self.value.with(cx.rt(), |v| *v).unwrap_or(self.last);
        let mut c = [0.0; N];
        for (i, s) in self.comps.iter().enumerate().take(N) {
            c[i] = s.get(cx.rt());
        }
        if v != self.last {
            for (i, s) in self.comps.iter().enumerate().take(N) {
                s.set(cx.rt_mut(), v[i]);
            }
            self.last = v;
        } else if c != self.last {
            self.value.set(cx.rt_mut(), c);
            self.last = c;
        }
        Handled::Yes
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
    }
}

// ---- quaternions -------------------------------------------------------------------------

/// Euler angles in degrees `(yaw about Y, pitch about X, roll about Z)`, applied yaw →
/// pitch → roll (`q = q_yaw · q_pitch · q_roll`).
pub fn quat_to_euler(q: DQuat) -> (f64, f64, f64) {
    let n = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    let (x, y, z, w) = if n > 0.0 {
        (q.x / n, q.y / n, q.z / n, q.w / n)
    } else {
        (0.0, 0.0, 0.0, 1.0)
    };
    let r00 = 1.0 - 2.0 * (y * y + z * z);
    let r02 = 2.0 * (x * z + y * w);
    let r10 = 2.0 * (x * y + z * w);
    let r11 = 1.0 - 2.0 * (x * x + z * z);
    let r12 = 2.0 * (y * z - x * w);
    let r20 = 2.0 * (x * z - y * w);
    let r22 = 1.0 - 2.0 * (x * x + y * y);
    let sp = (-r12).clamp(-1.0, 1.0);
    let pitch = sp.asin();
    let (yaw, roll) = if sp.abs() > 0.999_999 {
        // Gimbal lock: yaw and roll share an axis; put it all in yaw.
        ((-r20).atan2(r00), 0.0)
    } else {
        (r02.atan2(r22), r10.atan2(r11))
    };
    (yaw.to_degrees(), pitch.to_degrees(), roll.to_degrees())
}

/// The inverse of [`quat_to_euler`].
pub fn euler_to_quat(yaw: f64, pitch: f64, roll: f64) -> DQuat {
    let (sy, cy) = (yaw.to_radians() * 0.5).sin_cos();
    let (sp, cp) = (pitch.to_radians() * 0.5).sin_cos();
    let (sr, cr) = (roll.to_radians() * 0.5).sin_cos();
    // q_yaw (0, sy, 0, cy) · q_pitch (sp, 0, 0, cp) · q_roll (0, 0, sr, cr)
    let mul = |a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)| {
        (
            a.3 * b.0 + a.0 * b.3 + a.1 * b.2 - a.2 * b.1,
            a.3 * b.1 - a.0 * b.2 + a.1 * b.3 + a.2 * b.0,
            a.3 * b.2 + a.0 * b.1 - a.1 * b.0 + a.2 * b.3,
            a.3 * b.3 - a.0 * b.0 - a.1 * b.1 - a.2 * b.2,
        )
    };
    let q = mul(
        mul((0.0, sy, 0.0, cy), (sp, 0.0, 0.0, cp)),
        (0.0, 0.0, sr, cr),
    );
    DQuat::from_xyzw(q.0, q.1, q.2, q.3)
}

/// Pitch beyond which the Euler view is flagged (degrees from ±90°).
pub const GIMBAL_MARGIN_DEG: f64 = 1.0;

/// The parts of a quaternion editor.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct QuatParts {
    pub editor: WidgetId,
    pub yaw: WidgetId,
    pub pitch: WidgetId,
    pub roll: WidgetId,
    pub warning: WidgetId,
    pub raw: WidgetId,
}

/// Build a quaternion editor (Euler view with a gimbal warning).
pub fn quat_editor(
    b: &mut dyn Build,
    parent: WidgetId,
    key: impl Into<Key>,
    value: Signal<DQuat>,
    label: &str,
) -> Result<QuatParts, UiError> {
    let space = b.theme_ref().space;
    let q = value.get(b.runtime());
    let (y, p, r) = quat_to_euler(q);
    let yaw = b.signal(y);
    let pitch = b.signal(p);
    let roll = b.signal(r);
    let warning_text = b.signal(String::new());
    let raw_text = b.signal(raw(q));
    let editor = b.add(
        parent,
        key,
        NodeStyle::column(space[1]),
        QuatEditor {
            label: label.to_string(),
            value,
            euler: [yaw, pitch, roll],
            warning_text,
            raw_text,
            last_q: q,
            last_e: [y, p, r],
        },
    )?;
    let row = b.add(
        editor,
        "row",
        NodeStyle::row(space[1]),
        super::Container::group(),
    )?;
    let field = |b: &mut dyn Build, k: &'static str, s: Signal<f64>, name: &str| {
        b.add(
            row,
            k,
            NodeStyle::leaf().width(92.0),
            NumericField::new(s, &format!("{label} {name}"))
                .unit("deg")
                .decimals(2)
                .step(1.0),
        )
    };
    let yaw_id = field(b, "yaw", yaw, crate::tr!("yaw"))?;
    let pitch_id = field(b, "pitch", pitch, crate::tr!("pitch"))?;
    let roll_id = field(b, "roll", roll, crate::tr!("roll"))?;
    let warning = b.add(
        editor,
        "gimbal",
        NodeStyle::leaf().width(300.0),
        Label::new(warning_text).kind(LabelKind::Warning).wrapping(),
    )?;
    let initial = gimbal_text(p);
    b.hide(warning, initial.is_empty());
    warning_text.set(b.runtime(), initial);
    let raw_id = b.add(
        editor,
        "raw",
        NodeStyle::leaf(),
        Label::new(raw_text).kind(LabelKind::Small),
    )?;
    Ok(QuatParts {
        editor,
        yaw: yaw_id,
        pitch: pitch_id,
        roll: roll_id,
        warning,
        raw: raw_id,
    })
}

/// The gimbal warning for `pitch` (degrees), empty when the Euler view is faithful.
pub fn gimbal_text(pitch: f64) -> String {
    if 90.0 - pitch.abs() <= GIMBAL_MARGIN_DEG {
        crate::trf!(
            "⚠ Gimbal lock at pitch {pitch}°: yaw and roll now turn about the same axis, so the Euler view cannot represent every change. Rotate in the viewport or edit the quaternion.",
            pitch = format!("{pitch:.1}")
        )
    } else {
        String::new()
    }
}

fn raw(q: DQuat) -> String {
    crate::trf!(
        "quaternion x {x}  y {y}  z {z}  w {w}",
        x = format!("{:.4}", q.x),
        y = format!("{:.4}", q.y),
        z = format!("{:.4}", q.z),
        w = format!("{:.4}", q.w)
    )
}

/// The quaternion editor's root.
pub struct QuatEditor {
    label: String,
    value: Signal<DQuat>,
    euler: [Signal<f64>; 3],
    warning_text: Signal<String>,
    raw_text: Signal<String>,
    last_q: DQuat,
    last_e: [f64; 3],
}

impl QuatEditor {
    /// Whether the gimbal warning is showing.
    pub fn gimbal_warning(&self, rt: &crate::state::Runtime) -> bool {
        !self.warning_text.get(rt).is_empty()
    }
}

impl Widget for QuatEditor {
    fn role(&self) -> Role {
        Role::Group
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.value.any()), Dirty::NONE);
        for e in &self.euler {
            b.watch(Some(e.any()), Dirty::NONE);
        }
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if *ev != UiEvent::BindingChanged {
            return Handled::No;
        }
        let q = self.value.get(cx.rt());
        let e = [
            self.euler[0].get(cx.rt()),
            self.euler[1].get(cx.rt()),
            self.euler[2].get(cx.rt()),
        ];
        let q = if q != self.last_q {
            let (y, p, r) = quat_to_euler(q);
            for (s, v) in self.euler.iter().zip([y, p, r]) {
                s.set(cx.rt_mut(), v);
            }
            self.last_e = [y, p, r];
            self.last_q = q;
            q
        } else if e != self.last_e {
            let nq = euler_to_quat(e[0], e[1].clamp(-90.0, 90.0), e[2]);
            self.value.set(cx.rt_mut(), nq);
            self.last_q = nq;
            self.last_e = e;
            nq
        } else {
            q
        };
        let warn = gimbal_text(self.last_e[1]);
        let label = cx.id().child(&Key::Static("gimbal"));
        cx.set_hidden(label, warn.is_empty());
        self.warning_text.set(cx.rt_mut(), warn);
        self.raw_text.set(cx.rt_mut(), raw(q));
        Handled::Yes
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        let w = self.warning_text.get(cx.rt);
        if !w.is_empty() {
            node.set_description(w);
        }
    }
}

// ---- FramePos ----------------------------------------------------------------------------

/// Re-expresses a position in another frame (the frames the editor resolves; D-4: an
/// in-memory resolver until a plugin provides one).
pub type Rebaser = Rc<dyn Fn(FramePos, FrameId) -> Result<FramePos, String>>;

/// The parts of a `FramePos` editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FramePosParts {
    pub editor: WidgetId,
    pub frame: WidgetId,
    pub fields: [WidgetId; 3],
    pub note: WidgetId,
}

/// Build a `FramePos` editor over `frames` (id, display name).
pub fn frame_pos_editor(
    b: &mut dyn Build,
    parent: WidgetId,
    key: impl Into<Key>,
    value: Signal<FramePos>,
    frames: &[(FrameId, &str)],
    rebaser: Option<Rebaser>,
    label: &str,
) -> Result<FramePosParts, UiError> {
    let space = b.theme_ref().space;
    let init = value
        .with(b.runtime(), |v| *v)
        .unwrap_or(FramePos::origin_of(FrameId(0)));
    let idx = frames
        .iter()
        .position(|(f, _)| *f == init.frame)
        .unwrap_or(0);
    let frame_sel = b.signal(idx);
    let comps = [
        b.signal(init.local.x),
        b.signal(init.local.y),
        b.signal(init.local.z),
    ];
    let note = b.signal(String::new());
    let editor = b.add(
        parent,
        key,
        NodeStyle::column(space[1]),
        FramePosEditor {
            label: label.to_string(),
            value,
            frames: frames.iter().map(|(f, _)| *f).collect(),
            frame_sel,
            comps,
            note,
            rebaser,
            last: init,
        },
    )?;
    let row = b.add(
        editor,
        "row",
        NodeStyle::row(space[1]),
        super::Container::group(),
    )?;
    let names: Vec<&str> = frames.iter().map(|(_, n)| *n).collect();
    let frame = b.add(
        row,
        "frame",
        NodeStyle::leaf().width(150.0),
        ComboBox::new(&crate::trf!("{label} frame", label), &names, frame_sel),
    )?;
    let mut fields = [WidgetId(0); 3];
    for (i, c) in comps.iter().enumerate() {
        fields[i] = b.add(
            row,
            Key::Index(i as u32),
            NodeStyle::leaf().width(110.0),
            NumericField::new(*c, &format!("{label} {}", AXES[i]))
                .unit("m")
                .decimals(3),
        )?;
    }
    let note_id = b.add(
        editor,
        "note",
        NodeStyle::leaf().width(420.0),
        Label::new(note).kind(LabelKind::Small).wrapping(),
    )?;
    Ok(FramePosParts {
        editor,
        frame,
        fields,
        note: note_id,
    })
}

/// The `FramePos` editor's root.
pub struct FramePosEditor {
    label: String,
    value: Signal<FramePos>,
    frames: Vec<FrameId>,
    frame_sel: Signal<usize>,
    comps: [Signal<f64>; 3],
    note: Signal<String>,
    rebaser: Option<Rebaser>,
    last: FramePos,
}

impl FramePosEditor {
    fn push(&mut self, cx: &mut EventCx, p: FramePos) {
        if let Some(i) = self.frames.iter().position(|f| *f == p.frame) {
            self.frame_sel.set(cx.rt_mut(), i);
        }
        for (s, v) in self.comps.iter().zip([p.local.x, p.local.y, p.local.z]) {
            s.set(cx.rt_mut(), v);
        }
        self.last = p;
    }
}

impl Widget for FramePosEditor {
    fn role(&self) -> Role {
        Role::Group
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.value.any()), Dirty::NONE);
        b.watch(Some(self.frame_sel.any()), Dirty::NONE);
        for c in &self.comps {
            b.watch(Some(c.any()), Dirty::NONE);
        }
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if *ev != UiEvent::BindingChanged {
            return Handled::No;
        }
        let v = self.value.with(cx.rt(), |v| *v).unwrap_or(self.last);
        if v != self.last {
            self.push(cx, v);
            return Handled::Yes;
        }
        let frame = self
            .frames
            .get(self.frame_sel.get(cx.rt()))
            .copied()
            .unwrap_or(self.last.frame);
        let local = DVec3 {
            x: self.comps[0].get(cx.rt()),
            y: self.comps[1].get(cx.rt()),
            z: self.comps[2].get(cx.rt()),
        };
        if frame != self.last.frame {
            let next = match &self.rebaser {
                Some(rb) => match rb(self.last, frame) {
                    Ok(p) => {
                        self.note.set(
                            cx.rt_mut(),
                            crate::tr!("Re-expressed in the new frame: the point did not move.")
                                .into(),
                        );
                        p
                    }
                    Err(e) => {
                        // Refuse: keep the old frame, say why.
                        self.note.set(
                            cx.rt_mut(),
                            crate::trf!("Cannot re-express in that frame: {e}", e),
                        );
                        let last = self.last;
                        self.push(cx, last);
                        return Handled::Yes;
                    }
                },
                None => {
                    self.note.set(
                        cx.rt_mut(),
                        crate::tr!("No frame tree connected: the local offset was kept, so the point moved with the frame.").into(),
                    );
                    FramePos::new(frame, self.last.local)
                }
            };
            self.value.set(cx.rt_mut(), next);
            self.push(cx, next);
        } else if local != self.last.local {
            let next = FramePos::new(frame, local);
            self.value.set(cx.rt_mut(), next);
            self.last = next;
        }
        Handled::Yes
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euler_round_trips_away_from_the_pole() {
        for (y, p, r) in [
            (10.0, 20.0, 30.0),
            (-170.0, -45.0, 5.0),
            (0.0, 0.0, 0.0),
            (90.0, 80.0, -90.0),
        ] {
            let (y2, p2, r2) = quat_to_euler(euler_to_quat(y, p, r));
            assert!(
                (y - y2).abs() < 1e-9 && (p - p2).abs() < 1e-9 && (r - r2).abs() < 1e-9,
                "{y},{p},{r} -> {y2},{p2},{r2}"
            );
        }
        // At the pole the rotation (not the angles) survives.
        let q = euler_to_quat(30.0, 90.0, 20.0);
        let (y, p, r) = quat_to_euler(q);
        let q2 = euler_to_quat(y, p, r);
        let dot = (q.x * q2.x + q.y * q2.y + q.z * q2.z + q.w * q2.w).abs();
        assert!((dot - 1.0).abs() < 1e-9, "{q:?} vs {q2:?}");
    }
}
