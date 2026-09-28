//! Catalogue widget tests, part 5 (§21.16 "Editors"; DoD M2-24): colour picker, vector /
//! quaternion / `FramePos` editors, curve and gradient editors, asset reference picker,
//! property grid.

use std::rc::Rc;

use forge_frames::{DQuat, DVec3, FrameId, FramePos};
use forge_ui::color::HdrColor;
use forge_ui::dnd::{DragPayload, DropVerdict};
use forge_ui::testing::Harness;
use forge_ui::widgets::*;
use forge_ui::{Key, KeyCode, Modifiers, NodeStyle, Role, Signal, UiEvent, WidgetId};

const NONE: Modifiers = Modifiers::NONE;

fn host() -> (Harness, WidgetId) {
    Harness::with_host().unwrap_or_else(|e| panic!("{e}"))
}
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// Replace a numeric field's value by typing, as a user would.
fn type_into(h: &mut Harness, field: WidgetId, text: &str) {
    h.focus(field);
    h.press(KeyCode::Enter, NONE);
    h.press(KeyCode::Char('a'), Modifiers::CTRL);
    h.type_text(text);
    h.press(KeyCode::Enter, NONE);
}

// ---- colour picker -------------------------------------------------------------------------

#[test]
fn colour_picker_edits_hsv_alpha_and_exposure_in_linear_light() {
    let (mut h, host) = host();
    let c: Signal<HdrColor> =
        h.ui.rt_mut()
            .signal(HdrColor::from_srgb(1.0, 0.0, 0.0, 1.0));
    let p = color_picker(&mut h.ui, host, "cp", c, "Emission").unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    for id in [p.sat_val, p.hue, p.alpha] {
        assert_eq!(h.ui.role(id), Some(Role::Slider));
        assert!(h.ui.is_focusable(id));
    }
    assert_eq!(h.ui.role(p.hex), Some(Role::TextInput));
    assert_eq!(h.ui.role(p.swatch), Some(Role::ColorWell));
    // Saturation down: red towards white (green and blue rise together).
    h.focus(p.sat_val);
    h.press(KeyCode::Left, NONE);
    let v = c.get(h.ui.rt());
    assert!(v.g > 0.0 && close(v.g, v.b) && close(v.r, 1.0), "{v:?}");
    // Hue: 120 degrees on, red becomes green.
    h.press(KeyCode::Home, NONE); // saturation 0 ...
    h.press(KeyCode::End, NONE); // ... and back to 1: the hue survived the grey
    h.focus(p.hue);
    for _ in 0..4 {
        h.press(KeyCode::PageUp, NONE);
    }
    let v = c.get(h.ui.rt());
    assert!(v.g > 0.9 && v.r < 0.05, "hue 120: {v:?}");
    // Alpha.
    h.focus(p.alpha);
    h.press(KeyCode::Home, NONE);
    assert_eq!(c.get(h.ui.rt()).a, 0.0);
    h.press(KeyCode::End, NONE);
    // Exposure: +2 EV is four times brighter, still the same hue.
    type_into(&mut h, p.exposure, "2");
    let v = c.get(h.ui.rt());
    assert!(close(v.intensity(), 4.0) && v.is_hdr(), "{v:?}");
    let swatch = h
        .node(p.swatch)
        .and_then(|n| n.value().map(str::to_string))
        .unwrap_or_default();
    assert!(swatch.contains("HDR +2.00 EV"), "{swatch}");
    // The channel readout follows the Linear / sRGB switch.
    h.focus(p.mode);
    h.press(KeyCode::Right, NONE);
    let swatch = h
        .node(p.swatch)
        .and_then(|n| n.value().map(str::to_string))
        .unwrap_or_default();
    assert!(swatch.starts_with("sRGB"), "{swatch}");
}

#[test]
fn colour_picker_follows_outside_changes_and_refuses_bad_hex() {
    let (mut h, host) = host();
    let c = h.ui.rt_mut().signal(HdrColor::BLACK);
    let p = color_picker(&mut h.ui, host, "cp", c, "Tint").unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    // Undo (or another view) sets the colour: the parts re-derive, in the same frame.
    c.set(h.ui.rt_mut(), HdrColor::linear(0.0, 0.0, 8.0, 1.0));
    h.settle();
    let hue = h
        .node(p.hue)
        .and_then(|n| n.numeric_value())
        .unwrap_or(-1.0);
    assert!((hue - 2.0 / 3.0).abs() < 1e-3, "blue is hue 240 ({hue})");
    let ev = h
        .node(p.exposure)
        .and_then(|n| n.numeric_value())
        .unwrap_or(-1.0);
    assert!((ev - 3.0).abs() < 1e-3, "8.0 linear is +3 EV ({ev})");
    // Hex entry.
    type_into(&mut h, p.hex, "#ff8000");
    assert_eq!(c.get(h.ui.rt()).hex(), "#ff8000");
    type_into(&mut h, p.hex, "#12zz");
    assert_eq!(
        c.get(h.ui.rt()).hex(),
        "#ff8000",
        "a bad hex leaves the colour alone"
    );
    let err =
        h.ui.widget::<HexField>(p.hex)
            .and_then(|f| f.error().map(str::to_string))
            .unwrap_or_default();
    assert!(err.contains("not a hex colour"), "{err}");
    assert!(h.node(p.hex).is_some_and(|n| n.invalid().is_some()));
}

#[test]
fn colour_button_opens_a_picker_popover() {
    let (mut h, host) = host();
    let c = h.ui.rt_mut().signal(HdrColor::WHITE);
    let id =
        h.ui.add(host, "b", NodeStyle::leaf(), ColorButton::new(c, "Tint"))
            .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(id), Some(Role::ColorWell));
    h.focus(id);
    h.press(KeyCode::Enter, NONE);
    assert!(h.ui.widget::<ColorButton>(id).is_some_and(|b| b.is_open()));
    // Focus went into the picker (its first part); arrows edit the colour.
    h.press(KeyCode::Down, NONE);
    assert!(
        c.get(h.ui.rt()).r < 1.0,
        "the popover picker edits the bound colour"
    );
    h.press(KeyCode::Escape, NONE);
    assert!(!h.ui.widget::<ColorButton>(id).is_some_and(|b| b.is_open()));
    assert_eq!(h.ui.focused(), Some(id));
}

// ---- vector / quaternion / FramePos ----------------------------------------------------------

#[test]
fn vector_editor_round_trips_fields_and_model() {
    let (mut h, host) = host();
    let v: Signal<[f64; 3]> = h.ui.rt_mut().signal([1.0, 2.0, 3.0]);
    let p = vector_editor::<3>(&mut h.ui, host, "v", v, "Scale", Some("m"))
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(p.fields.len(), 3);
    assert_eq!(h.ui.role(p.fields[1]), Some(Role::SpinButton));
    assert_eq!(
        h.node(p.fields[1])
            .and_then(|n| n.label().map(str::to_string))
            .as_deref(),
        Some("Scale Y")
    );
    type_into(&mut h, p.fields[1], "250 cm");
    assert_eq!(
        v.with(h.ui.rt(), |x| *x),
        Some([1.0, 2.5, 3.0]),
        "typed with a unit, converted, f64"
    );
    v.set(h.ui.rt_mut(), [7.0, 8.0, 9.0]);
    h.settle();
    assert_eq!(
        h.node(p.fields[2]).and_then(|n| n.numeric_value()),
        Some(9.0)
    );
}

#[test]
fn quaternion_editor_edits_through_euler_and_warns_at_the_pole() {
    let (mut h, host) = host();
    let q = h.ui.rt_mut().signal(DQuat::IDENTITY);
    let p = quat_editor(&mut h.ui, host, "q", q, "Rotation").unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert!(h.ui.is_hidden(p.warning), "no warning away from the pole");
    type_into(&mut h, p.yaw, "90");
    let got = q.get(h.ui.rt());
    let s = std::f64::consts::FRAC_1_SQRT_2;
    assert!(
        (got.y - s).abs() < 1e-9 && (got.w - s).abs() < 1e-9,
        "yaw 90 about Y: {got:?}"
    );
    type_into(&mut h, p.pitch, "90");
    assert!(!h.ui.is_hidden(p.warning), "gimbal warning at pitch 90");
    assert!(
        h.ui.widget::<QuatEditor>(p.editor)
            .is_some_and(|e| e.gimbal_warning(h.ui.rt()))
    );
    let desc = h
        .node(p.editor)
        .and_then(|n| n.description().map(str::to_string))
        .unwrap_or_default();
    assert!(desc.contains("Gimbal lock"), "announced: {desc}");
    // An outside change refreshes the Euler fields.
    q.set(h.ui.rt_mut(), euler_to_quat(10.0, 20.0, 30.0));
    h.settle();
    let yaw = h.node(p.yaw).and_then(|n| n.numeric_value()).unwrap_or(0.0);
    assert!((yaw - 10.0).abs() < 1e-6, "{yaw}");
    assert!(h.ui.is_hidden(p.warning));
}

#[test]
fn frame_pos_editor_is_f64_and_never_silently_moves_a_point() {
    let (mut h, host) = host();
    let start = FramePos::new(
        FrameId(1),
        DVec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
    );
    let pos = h.ui.rt_mut().signal(start);
    let frames = [(FrameId(0), "System"), (FrameId(1), "Surface")];
    let p = frame_pos_editor(&mut h.ui, host, "fp", pos, &frames, None, "Position")
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(p.frame), Some(Role::ComboBox));
    // f64 precision survives a round trip through the field (an f32 would not).
    type_into(&mut h, p.fields[0], "123456789.123");
    assert_eq!(pos.with(h.ui.rt(), |v| v.local.x), Some(123_456_789.123));
    // Frame change without a rebaser: kept local offset, and the note says so.
    h.focus(p.frame);
    h.press(KeyCode::Enter, NONE);
    h.press(KeyCode::Home, Modifiers::CTRL);
    h.press(KeyCode::Enter, NONE);
    let now = pos.with(h.ui.rt(), |v| *v).unwrap_or(start);
    assert_eq!(now.frame, FrameId(0));
    assert_eq!(now.local.y, 2.0);
    let note = h
        .node(p.note)
        .and_then(|n| n.label().map(str::to_string))
        .unwrap_or_default();
    assert!(note.contains("local offset was kept"), "{note}");

    // With a rebaser (the frame tree): the point is re-expressed and does not move.
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let pos = h.ui.rt_mut().signal(start);
    let rebaser: Rebaser = Rc::new(|p: FramePos, to: FrameId| {
        // Surface (1) sits at +100 on x in System (0).
        let dx = match (p.frame, to) {
            (FrameId(1), FrameId(0)) => 100.0,
            (FrameId(0), FrameId(1)) => -100.0,
            _ => 0.0,
        };
        Ok(FramePos::new(
            to,
            DVec3 {
                x: p.local.x + dx,
                ..p.local
            },
        ))
    });
    let p = frame_pos_editor(
        &mut h.ui,
        host,
        "fp",
        pos,
        &frames,
        Some(rebaser),
        "Position",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    h.focus(p.frame);
    h.press(KeyCode::Enter, NONE);
    h.press(KeyCode::Home, Modifiers::CTRL);
    h.press(KeyCode::Enter, NONE);
    let now = pos.with(h.ui.rt(), |v| *v).unwrap_or(start);
    assert_eq!((now.frame, now.local.x), (FrameId(0), 101.0));
    assert_eq!(
        h.node(p.fields[0]).and_then(|n| n.numeric_value()),
        Some(101.0),
        "fields follow"
    );
}

// ---- curve / gradient ----------------------------------------------------------------------

fn curve() -> Curve {
    Curve::new(vec![
        CurveKey {
            t: 0.0,
            v: 0.0,
            interp: Interp::Linear,
        },
        CurveKey {
            t: 1.0,
            v: 1.0,
            interp: Interp::Linear,
        },
    ])
}

#[test]
fn curve_editor_edits_keys_by_keyboard() {
    let (mut h, host) = host();
    let c = h.ui.rt_mut().signal(curve());
    let id =
        h.ui.add(
            host,
            "c",
            NodeStyle::leaf().size(300.0, 150.0),
            CurveEditor::new(c, "Falloff"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(id), Some(Role::ListBox));
    assert_eq!(
        h.child_node(id, 1).map(|n| n.role()),
        Some(Role::ListBoxOption)
    );
    h.focus(id);
    h.press(KeyCode::Up, NONE);
    assert!(
        (c.get(h.ui.rt()).keys[0].v - 0.01).abs() < 1e-12,
        "Up raises the selected key"
    );
    h.press(KeyCode::Insert, NONE);
    let k = c.get(h.ui.rt()).keys;
    assert_eq!(k.len(), 3);
    assert!((k[1].t - 0.5).abs() < 1e-12, "inserted halfway");
    assert!(
        (k[1].v - c.get(h.ui.rt()).eval(0.5)).abs() < 1e-12,
        "on the curve"
    );
    h.press(KeyCode::Enter, NONE);
    assert_eq!(
        c.get(h.ui.rt()).keys[1].interp,
        Interp::Constant,
        "Enter cycles the interpolation"
    );
    h.press(KeyCode::Right, Modifiers::CTRL);
    h.press(KeyCode::Left, NONE);
    let k = c.get(h.ui.rt()).keys;
    assert!((k[2].t - 0.99).abs() < 1e-12, "Left moves the key in time");
    for _ in 0..200 {
        h.press(KeyCode::Left, NONE);
    }
    let k = c.get(h.ui.rt()).keys;
    assert!(k[2].t > k[1].t, "a key never passes its neighbour");
    h.press(KeyCode::Delete, NONE);
    h.press(KeyCode::Delete, NONE);
    h.press(KeyCode::Delete, NONE);
    assert_eq!(c.get(h.ui.rt()).keys.len(), 1, "one key always stays");
    assert!(
        h.take::<CurveEdited>().len() >= 4,
        "one CurveEdited per edit"
    );
}

#[test]
fn curve_editor_drags_a_key_as_one_gesture() {
    let (mut h, host) = host();
    let c = h.ui.rt_mut().signal(curve());
    let id =
        h.ui.add(
            host,
            "c",
            NodeStyle::leaf().size(300.0, 150.0),
            CurveEditor::new(c, "Falloff"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let r = h.ui.rect(id).unwrap_or_default();
    // Key 0 is at the plot's bottom-left (padding 10).
    let from = forge_ui::Point::new(r.x + 10.0, r.bottom() - 10.0);
    h.take::<CurveEdited>();
    h.drag(
        from,
        forge_ui::Point::new(r.x + 10.0, r.y + 10.0 + (r.h - 20.0) * 0.5),
        10,
    );
    assert!(
        (c.get(h.ui.rt()).keys[0].v - 0.5).abs() < 0.02,
        "{:?}",
        c.get(h.ui.rt()).keys[0]
    );
    assert_eq!(h.take::<CurveEdited>().len(), 1, "a drag is one edit");
}

#[test]
fn gradient_editor_moves_adds_removes_and_recolours_stops() {
    let (mut h, host) = host();
    let g = h.ui.rt_mut().signal(Gradient::new(vec![
        GradientStop {
            pos: 0.0,
            color: HdrColor::linear(1.0, 0.0, 0.0, 1.0),
        },
        GradientStop {
            pos: 1.0,
            color: HdrColor::linear(0.0, 0.0, 1.0, 1.0),
        },
    ]));
    let scratch = h.ui.rt_mut().signal(HdrColor::default());
    let id =
        h.ui.add(
            host,
            "g",
            NodeStyle::leaf().width(300.0),
            GradientEditor::new(g, scratch, "Sky"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(id), Some(Role::ListBox));
    h.focus(id);
    h.press(KeyCode::Right, NONE);
    assert!(close(g.get(h.ui.rt()).stops[0].pos, 0.01));
    h.press(KeyCode::Insert, NONE);
    let s = g.get(h.ui.rt()).stops;
    assert_eq!(s.len(), 3);
    assert!(
        close(s[1].color.r, 0.5) && close(s[1].color.b, 0.5),
        "interpolated in linear light: {:?}",
        s[1].color
    );
    h.press(KeyCode::Delete, NONE);
    h.press(KeyCode::Delete, NONE);
    assert_eq!(g.get(h.ui.rt()).stops.len(), 2, "two stops always stay");
    // Enter opens a picker for the selected stop; editing it recolours the stop.
    h.press(KeyCode::Home, NONE);
    h.press(KeyCode::Enter, NONE);
    assert!(
        h.ui.widget::<GradientEditor>(id)
            .is_some_and(|e| e.is_picking())
    );
    h.press(KeyCode::Left, NONE); // saturation down in the popover's surface
    let c0 = g.get(h.ui.rt()).stops[0].color;
    assert!(c0.g > 0.0, "the stop took the picker's colour: {c0:?}");
    h.take::<GradientEdited>();
    h.press(KeyCode::Escape, NONE);
    assert_eq!(
        h.take::<GradientEdited>().len(),
        1,
        "closing the picker ends the edit"
    );
    assert_eq!(
        h.child_node(id, 0).map(|n| n.role()),
        Some(Role::ListBoxOption)
    );
}

// ---- asset reference picker ----------------------------------------------------------------

fn index() -> Rc<dyn AssetIndex> {
    Rc::new(MemoryAssetIndex::new(&[
        ("textures/rock.tex", "Texture"),
        ("textures/grass.tex", "Texture"),
        ("meshes/boulder.mesh", "Mesh"),
    ]))
}

#[test]
fn asset_picker_picks_clears_and_filters_by_kind() {
    let (mut h, host) = host();
    let v = h.ui.rt_mut().signal(None::<String>);
    let scratch = h.ui.rt_mut().signal(None::<String>);
    let id =
        h.ui.add(
            host,
            "a",
            NodeStyle::leaf().width(240.0),
            AssetRefPicker::new(v, scratch, "Texture", index(), "Albedo"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(id), Some(Role::ComboBox));
    h.focus(id);
    h.press(KeyCode::Enter, NONE);
    let list = h.ui.popups()[0];
    assert_eq!(
        h.ui.widget::<PickList>(list).map(|l| l.shown().len()),
        Some(2),
        "only Textures are offered"
    );
    h.type_text("gra");
    h.press(KeyCode::Enter, NONE);
    assert_eq!(v.get(h.ui.rt()).as_deref(), Some("textures/grass.tex"));
    assert_eq!(h.take::<AssetRefChanged>().len(), 1);
    assert_eq!(
        h.node(id)
            .and_then(|n| n.value().map(str::to_string))
            .as_deref(),
        Some("textures/grass.tex")
    );
    h.press(KeyCode::Delete, NONE);
    assert_eq!(v.get(h.ui.rt()), None);
}

#[test]
fn asset_picker_accepts_its_kind_and_refuses_others_with_a_reason() {
    let (mut h, host) = host();
    let v = h.ui.rt_mut().signal(None::<String>);
    let scratch = h.ui.rt_mut().signal(None::<String>);
    let id =
        h.ui.add(
            host,
            "a",
            NodeStyle::leaf().width(240.0),
            AssetRefPicker::new(v, scratch, "Texture", index(), "Albedo"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let p =
        h.ui.widget::<AssetRefPicker>(id)
            .unwrap_or_else(|| panic!("picker"));
    assert_eq!(p.verdict("textures/rock.tex"), DropVerdict::Accepted);
    match p.verdict("meshes/boulder.mesh") {
        DropVerdict::Refused(why) => {
            assert!(why.contains("Texture") && why.contains("Mesh"), "{why}")
        }
        other => panic!("{other:?}"),
    }
    h.ui.send(
        id,
        &UiEvent::Drop(DragPayload::Asset("meshes/boulder.mesh".into())),
    );
    assert_eq!(v.get(h.ui.rt()), None, "a refused drop changes nothing");
    h.ui.send(
        id,
        &UiEvent::Drop(DragPayload::Asset("textures/rock.tex".into())),
    );
    h.settle();
    assert_eq!(v.get(h.ui.rt()).as_deref(), Some("textures/rock.tex"));
}

// ---- property grid -------------------------------------------------------------------------

#[test]
fn property_grid_builds_editors_per_kind_and_filters_rows() {
    let (mut h, host) = host();
    let name = h.ui.rt_mut().signal(String::from("Boulder"));
    let vis = h.ui.rt_mut().signal(true);
    let mass = h.ui.rt_mut().signal(12.0f64);
    let body = h.ui.rt_mut().signal(0usize);
    let tint = h.ui.rt_mut().signal(HdrColor::WHITE);
    let scale = h.ui.rt_mut().signal([1.0f64, 1.0, 1.0]);
    let rot = h.ui.rt_mut().signal(DQuat::IDENTITY);
    let tex = h.ui.rt_mut().signal(None::<String>);
    let id_text = h.ui.rt_mut().signal(String::from("#4711"));
    let parts = property_grid(
        &mut h.ui,
        host,
        "pg",
        "Inspector",
        vec![
            Property::new("General", "Name", PropKind::Text(name)),
            Property::new("General", "Visible", PropKind::Bool(vis)),
            Property::new("General", "Id", PropKind::ReadOnly(id_text)),
            Property::new(
                "Physics",
                "Mass",
                PropKind::Number {
                    value: mass,
                    unit: Some("kg".into()),
                    range: None,
                },
            ),
            Property::new(
                "Physics",
                "Body",
                PropKind::Choice {
                    options: vec!["Static".into(), "Dynamic".into()],
                    selected: body,
                },
            ),
            Property::new(
                "Transform",
                "Scale",
                PropKind::Vec3 {
                    value: scale,
                    unit: None,
                },
            ),
            Property::new("Transform", "Rotation", PropKind::Rotation(rot)),
            Property::new("Render", "Tint", PropKind::Color(tint)),
            Property::new(
                "Render",
                "Albedo",
                PropKind::Asset {
                    value: tex,
                    kind: "Texture".into(),
                    index: index(),
                },
            ),
        ],
    )
    .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let role_of = |h: &Harness, n: &str| {
        parts
            .rows
            .iter()
            .find(|r| r.0 == n)
            .and_then(|r| h.ui.role(r.2))
    };
    assert_eq!(role_of(&h, "Name"), Some(Role::TextInput));
    assert_eq!(role_of(&h, "Visible"), Some(Role::CheckBox));
    assert_eq!(role_of(&h, "Id"), Some(Role::Label));
    assert_eq!(role_of(&h, "Mass"), Some(Role::SpinButton));
    assert_eq!(role_of(&h, "Body"), Some(Role::ComboBox));
    assert_eq!(role_of(&h, "Scale"), Some(Role::Group));
    assert_eq!(role_of(&h, "Tint"), Some(Role::ColorWell));
    assert_eq!(role_of(&h, "Albedo"), Some(Role::ComboBox));
    assert_eq!(parts.categories.len(), 4);
    // Each row is a group named by its property, so the editor is announced with it.
    let (_, row, ed) = parts
        .rows
        .iter()
        .find(|r| r.0 == "Mass")
        .cloned()
        .unwrap_or_else(|| panic!("row"));
    assert_eq!(
        h.node(row)
            .and_then(|n| n.label().map(str::to_string))
            .as_deref(),
        Some("Mass")
    );
    // Editing through the grid edits the bound signal.
    type_into(&mut h, ed, "3 Mg");
    assert_eq!(mass.get(h.ui.rt()), 3000.0);
    // The filter hides rows that do not match, and empty categories.
    h.focus(parts.filter);
    h.type_text("mas");
    h.press(KeyCode::Enter, NONE);
    let hidden = |h: &Harness, n: &str| {
        parts
            .rows
            .iter()
            .find(|r| r.0 == n)
            .is_some_and(|r| h.ui.is_hidden(r.1))
    };
    assert!(!hidden(&h, "Mass"));
    assert!(hidden(&h, "Name") && hidden(&h, "Tint"));
    let render_head = parts
        .categories
        .iter()
        .find(|c| c.0 == "Render")
        .map(|c| c.1)
        .unwrap_or(WidgetId(0));
    assert!(h.ui.is_hidden(render_head), "an empty category hides");
    h.press(KeyCode::Escape, NONE);
    h.advance(SEARCH_DEBOUNCE + std::time::Duration::from_millis(5));
    assert!(!hidden(&h, "Name"), "clearing the filter shows every row");
    let _ = Key::Index(0);
}
