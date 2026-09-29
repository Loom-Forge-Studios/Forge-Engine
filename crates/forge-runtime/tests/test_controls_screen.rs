//! Guard `C-input-game-ui` (Ch.28 §28.13, §28.15, §28.17; DoD M7-12): the game-facing input
//! UI, headless on `forge-ui` with virtual devices.
//!
//! * The rebinding screen shows every action's keyboard and gamepad bindings as the pad
//!   family's glyphs (a composite one button per direction); pressing one and then a pad
//!   button rebinds it, the swap is named, and the change is **saved at once**: a restarted
//!   game (a fresh runtime loading the profile) plays the new binding. Every visible string
//!   of the screen is a localisation key (the pseudo-locale transforms them all).
//! * The on-screen controls' view turns a mouse drag on the stick into the pad's stick.
//! * The menus' gamepad source reads the player's pad through the runtime.
//!
//! Positive control (W2): `positive_control_a_screen_that_does_not_save_loses_the_rebinding`
//! — the same flow with no profile store: the restarted game has lost the rebinding.

use std::cell::RefCell;
use std::rc::Rc;

use forge_input::{
    ActionDef, ActionKind, Binding, BindingDef, ContextDef, ControlRef, Device, DeviceDesc,
    DeviceId, GlyphContext, InputMapDef, InputRuntime, MemoryOverrideStore, OnScreenControls,
    OverrideStore, PadFamily, PlayerId,
};
use forge_runtime::input_ui::{
    Column, ControlsScreen, InputPadSource, OnScreenControlsView, controls_strings,
};
use forge_ui::game::{GamepadSource, GlyphSet, PadButton, PadInput};
use forge_ui::l10n::{PSEUDO_LOCALE, is_pseudo};
use forge_ui::testing::Harness;
use forge_ui::widgets::Pressed;
use forge_ui::{InputEvent, NodeStyle, Point, PointerButton, Size, UiConfig};

const P0: PlayerId = PlayerId(0);
const DT: f32 = 1.0 / 60.0;

fn map() -> InputMapDef {
    let c = |d, n: &str| ControlRef::new(d, n);
    InputMapDef::new().context(
        ContextDef::new("play")
            .action(
                ActionDef::new("jump", ActionKind::Button)
                    .bind(BindingDef::new(Binding::Control(c(
                        Device::Gamepad,
                        "South",
                    ))))
                    .bind(BindingDef::new(Binding::Control(c(
                        Device::Keyboard,
                        "Space",
                    )))),
            )
            .action(
                ActionDef::new("interact", ActionKind::Button).bind(BindingDef::new(
                    Binding::Control(c(Device::Gamepad, "North")),
                )),
            )
            .action(
                ActionDef::new("move", ActionKind::Axis2D)
                    .bind(BindingDef::new(Binding::Control(c(
                        Device::Gamepad,
                        "LeftStick",
                    ))))
                    .bind(BindingDef::new(Binding::Composite2D([
                        c(Device::Keyboard, "W"),
                        c(Device::Keyboard, "S"),
                        c(Device::Keyboard, "A"),
                        c(Device::Keyboard, "D"),
                    ]))),
            ),
    )
}

fn runtime() -> (InputRuntime, DeviceId) {
    let mut rt = InputRuntime::new();
    assert!(rt.set_map(&map()).is_empty());
    let pad = rt.connect(
        DeviceDesc::new(Device::Gamepad, "Virtual pad", "vpad")
            .family(PadFamily::PlayStation)
            .virtual_device(),
    );
    rt.connect(DeviceDesc::new(Device::Keyboard, "Keyboard", "kb").virtual_device());
    (rt, pad)
}

fn harness() -> Harness {
    Harness::new(UiConfig {
        size: Size::new(1280.0, 720.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"))
}

fn glyphs() -> GlyphContext {
    GlyphContext {
        family: PadFamily::PlayStation,
        ..GlyphContext::default()
    }
}

fn press(h: &mut Harness, s: &mut ControlsScreen, rt: &mut InputRuntime, id: forge_ui::WidgetId) {
    h.ui.raise(id, Pressed(id));
    h.settle();
    let acts = h.ui.take_actions();
    s.on_actions(&mut h.ui, rt, &acts);
}

/// Rebind Jump's gamepad button to the pad's West through the screen; then "restart" and
/// report whether West jumps.
fn rebind_and_restart(store: Option<&MemoryOverrideStore>) -> Result<(), String> {
    let (mut rt, pad) = runtime();
    let mut h = harness();
    let root = h.ui.root();
    let mut s = ControlsScreen::build(&mut h.ui, root, &rt, P0, controls_strings(), glyphs())
        .map_err(|e| e.to_string())?;
    h.settle();
    let jump = rt.handle("play/jump").ok_or("no jump")?;
    let b = s.button(jump, Column::Gamepad).ok_or("no gamepad button")?;
    if s.label_of(&h.ui, b).as_deref() != Some("\u{2715}") {
        return Err(format!(
            "Jump's pad glyph is {:?}, not the cross",
            s.label_of(&h.ui, b)
        ));
    }
    press(&mut h, &mut s, &mut rt, b);
    if !s.status(&h.ui).contains("jump") {
        return Err(format!(
            "no prompt naming the action: {:?}",
            s.status(&h.ui)
        ));
    }
    rt.send(pad, "West", [1.0, 0.0]);
    rt.update(DT);
    let st = store.map(|m| (m as &dyn OverrideStore, "player1"));
    s.update(&mut h.ui, &mut rt, st);
    rt.send(pad, "West", [0.0, 0.0]);
    rt.update(DT);
    h.settle();
    if s.label_of(&h.ui, b).as_deref() != Some("\u{25a1}") {
        return Err(format!(
            "the button shows {:?} after the rebinding",
            s.label_of(&h.ui, b)
        ));
    }
    // A restart: a fresh runtime loads the player's profile.
    let (mut rt2, pad2) = runtime();
    if let Some(m) = store {
        rt2.load_bindings(P0, m, "player1")
            .map_err(|e| e.to_string())?;
    }
    rt2.send(pad2, "West", [1.0, 0.0]);
    rt2.update(DT);
    let j2 = rt2.handle("play/jump").ok_or("no jump")?;
    if !rt2.action(P0, j2).triggered {
        return Err("after a restart West does not jump: the rebinding was lost".into());
    }
    Ok(())
}

#[test]
fn a_rebinding_on_the_screen_is_saved_and_survives_a_restart() {
    let store = MemoryOverrideStore::new();
    rebind_and_restart(Some(&store)).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_screen_that_does_not_save_loses_the_rebinding() {
    let e = rebind_and_restart(None).unwrap_err();
    assert!(e.contains("lost"), "{e}");
}

#[test]
fn a_conflict_is_swapped_and_named_and_composites_rebind_per_direction() {
    let (mut rt, pad) = runtime();
    let mut h = harness();
    let root = h.ui.root();
    let mut s = ControlsScreen::build(&mut h.ui, root, &rt, P0, controls_strings(), glyphs())
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let (jump, interact, mv) = (
        rt.handle("play/jump").unwrap_or_else(|| panic!()),
        rt.handle("play/interact").unwrap_or_else(|| panic!()),
        rt.handle("play/move").unwrap_or_else(|| panic!()),
    );
    // Jump takes North, which Interact had: Interact gets Jump's old South.
    let b = s.button(jump, Column::Gamepad).unwrap_or_else(|| panic!());
    press(&mut h, &mut s, &mut rt, b);
    rt.send(pad, "North", [1.0, 0.0]);
    rt.update(DT);
    s.update(&mut h.ui, &mut rt, None);
    assert!(s.status(&h.ui).contains("interact"), "{}", s.status(&h.ui));
    let bi = s
        .button(interact, Column::Gamepad)
        .unwrap_or_else(|| panic!());
    assert_eq!(s.label_of(&h.ui, bi).as_deref(), Some("\u{2715}"));
    // WASD shows four buttons; rebinding "up" to I leaves the others.
    let parts = s.buttons(mv, Column::KeyboardMouse);
    assert_eq!(parts.len(), 4);
    let labels: Vec<_> = parts
        .iter()
        .map(|p| s.label_of(&h.ui, *p).unwrap_or_default())
        .collect();
    assert_eq!(labels, ["W", "S", "A", "D"]);
    press(&mut h, &mut s, &mut rt, parts[0]);
    let kb = rt
        .devices()
        .find(|(_, d, _)| d.class == Device::Keyboard)
        .map(|(id, ..)| id)
        .unwrap_or_else(|| panic!());
    // A pad press is not for the keyboard column: ignored, still listening.
    rt.send(pad, "East", [1.0, 0.0]);
    rt.update(DT);
    s.update(&mut h.ui, &mut rt, None);
    rt.send(kb, "I", [1.0, 0.0]);
    rt.update(DT);
    s.update(&mut h.ui, &mut rt, None);
    let labels: Vec<_> = parts
        .iter()
        .map(|p| s.label_of(&h.ui, *p).unwrap_or_default())
        .collect();
    assert_eq!(labels, ["I", "S", "A", "D"]);
    // Reset brings every default back.
    let reset = s.reset_button();
    press(&mut h, &mut s, &mut rt, reset);
    let labels: Vec<_> = parts
        .iter()
        .map(|p| s.label_of(&h.ui, *p).unwrap_or_default())
        .collect();
    assert_eq!(labels, ["W", "S", "A", "D"]);
    assert_eq!(s.label_of(&h.ui, b).as_deref(), Some("\u{2715}"));
}

#[test]
fn every_visible_string_of_the_screen_is_localised() {
    let (rt, _) = runtime();
    let mut h = harness();
    let root = h.ui.root();
    let mut loc = controls_strings();
    loc.set_current(PSEUDO_LOCALE);
    let s = ControlsScreen::build(&mut h.ui, root, &rt, P0, loc, glyphs())
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let mut texts = Vec::new();
    for (id, _) in h.ui.walk() {
        if let Some(l) = h.node(id).and_then(|n| n.label().map(str::to_string))
            && !l.is_empty()
        {
            texts.push(l);
        }
    }
    // Chrome strings are pseudo-localised; action names are the authored names (data), and
    // glyphs are key and button names.
    for want in [
        "Controls",
        "Keyboard and mouse",
        "Gamepad",
        "Reset to defaults",
    ] {
        assert!(
            !texts.iter().any(|t| t == want),
            "{want:?} shown untranslated among {texts:?}"
        );
    }
    assert!(
        texts.iter().filter(|t| is_pseudo(t)).count() >= 4,
        "the chrome is not pseudo-localised: {texts:?}"
    );
    let _ = s;
}

#[test]
fn the_on_screen_view_drives_the_pad_stick_with_a_mouse_drag() {
    let (rt, _) = runtime();
    let rt = Rc::new(RefCell::new(rt));
    let os = Rc::new(RefCell::new(OnScreenControls::new(
        &mut rt.borrow_mut(),
        OnScreenControls::default_layout(),
    )));
    let mut h = harness();
    let root = h.ui.root();
    let loc = controls_strings();
    let view =
        h.ui.add(
            root,
            "onscreen",
            NodeStyle::default().fill(),
            OnScreenControlsView::new(os.clone(), rt.clone(), &loc),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let r = h.ui.rect(view).unwrap_or_else(|| panic!("no rect"));
    let c = {
        let mut o = os.borrow_mut();
        o.set_screen(r.w, r.h);
        o.center(0)
    };
    let at = |x: f32, y: f32| Point::new(r.x + x, r.y + y);
    h.ui.handle(InputEvent::PointerMoved(at(c[0], c[1])));
    h.ui.handle(InputEvent::PointerButton {
        pos: at(c[0], c[1]),
        button: PointerButton::Primary,
        pressed: true,
    });
    h.ui.handle(InputEvent::PointerMoved(at(c[0] + 200.0, c[1])));
    h.settle();
    let mv = rt.borrow().handle("play/move").unwrap_or_else(|| panic!());
    rt.borrow_mut().update(DT);
    let v = rt.borrow().action(P0, mv).value;
    assert!((v[0] - 1.0).abs() < 1e-3 && v[1].abs() < 1e-3, "{v:?}");
    h.ui.handle(InputEvent::PointerButton {
        pos: at(c[0] + 200.0, c[1]),
        button: PointerButton::Primary,
        pressed: false,
    });
    rt.borrow_mut().update(DT);
    assert_eq!(rt.borrow().action(P0, mv).value, [0.0, 0.0]);
    let node = h.node(view).unwrap_or_else(|| panic!("no a11y node"));
    assert_eq!(node.label(), Some("On-screen controls"));
}

#[test]
fn the_menus_read_the_players_pad_through_the_runtime() {
    let (rt, pad) = runtime();
    let rt = Rc::new(RefCell::new(rt));
    let mut src = InputPadSource::new(rt.clone(), P0);
    assert_eq!(src.glyphs(), GlyphSet::PlayStation);
    rt.borrow_mut().send(pad, "South", [1.0, 0.0]);
    rt.borrow_mut().send(pad, "LeftStick", [0.0, 0.9]);
    rt.borrow_mut().update(DT);
    let got = src.poll();
    assert!(got.contains(&PadInput::Button {
        button: PadButton::South,
        pressed: true
    }));
    assert!(got.contains(&PadInput::Stick { x: 0.0, y: 0.9 }));
    assert!(src.poll().is_empty(), "an unchanged pad reported again");
}
