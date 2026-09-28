//! The sample game UI (Ch.21 §21.20, DoD M2-29), headless: a pad walks the main menu, the
//! settings screen (sliders and switches adjust with Left/Right, Up/Down move between rows),
//! the credits (the E-64 entry, static text), the HUD and the pause menu; a language switch
//! relabels everything (the pseudo-locale transforms every visible menu string); what the
//! player asks for reaches a `bevy_ecs` world as messages; an idle menu costs nothing.

use std::time::Duration;

use forge_core::bevy_ecs::message::Messages;
use forge_runtime::game_ui::{GameEvent, GameMenu, GameUiConfig, Screen};
use forge_ui::game::{ENGINE_CREDIT, GlyphSet, PadButton, PadInput, REPEAT_DELAY, REPEAT_EVERY};
use forge_ui::l10n::is_pseudo;
use forge_ui::testing::Harness;
use forge_ui::widgets::{Label, Progress};
use forge_ui::{Key, KeyCode, Modifiers, Role, Size, UiConfig, WidgetId};

fn setup() -> (Harness, GameMenu) {
    let mut h = Harness::new(UiConfig {
        size: Size::new(1280.0, 720.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let m = GameMenu::build(&mut h.ui, GameUiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    (h, m)
}

fn at(id: WidgetId, path: &[&'static str]) -> WidgetId {
    path.iter().fold(id, |w, k| w.child(&Key::Static(k)))
}

fn tap(h: &mut Harness, m: &mut GameMenu, b: PadButton) {
    let now = h.now();
    m.pad(
        &mut h.ui,
        now,
        PadInput::Button {
            button: b,
            pressed: true,
        },
    );
    m.pad(
        &mut h.ui,
        now,
        PadInput::Button {
            button: b,
            pressed: false,
        },
    );
    h.settle();
}

fn focused_is(h: &Harness, id: WidgetId, what: &str) {
    let f = h.ui.focused();
    assert_eq!(
        f,
        Some(id),
        "focus should be on {what}; it is on {:?} at {:?}, the target at {:?}",
        f.and_then(|f| h.ui.key_of(f).cloned()),
        f.and_then(|f| h.ui.rect(f)),
        h.ui.rect(id)
    );
}

#[test]
fn a_pad_walks_every_screen() {
    let (mut h, mut m) = setup();
    let main = m.screen_id(Screen::Main);
    assert_eq!(m.screen(), Screen::Main);
    focused_is(
        &h,
        at(main, &["play"]),
        "Play (a game menu always has focus)",
    );
    tap(&mut h, &mut m, PadButton::DPadDown);
    focused_is(&h, at(main, &["settings"]), "Settings");
    tap(&mut h, &mut m, PadButton::South);
    assert_eq!(m.screen(), Screen::Settings);
    let body = at(m.screen_id(Screen::Settings), &["body"]);
    focused_is(
        &h,
        at(body, &["master", "control"]),
        "the master volume slider",
    );
    // Left/Right adjust the focused slider; the game hears one settings change.
    let before = m.settings().master_volume;
    tap(&mut h, &mut m, PadButton::DPadRight);
    m.update(&mut h.ui);
    assert!(
        (m.settings().master_volume - (before + 0.05)).abs() < 1e-4,
        "{}",
        m.settings().master_volume
    );
    let evs = m.take_events();
    assert!(
        matches!(evs.as_slice(), [GameEvent::SettingsChanged(s)] if (s.master_volume - before - 0.05).abs() < 1e-4),
        "{evs:?}"
    );
    focused_is(&h, at(body, &["master", "control"]), "the slider still");
    // Up/Down move between rows even on a slider (game_nav): down to fullscreen, toggle it.
    tap(&mut h, &mut m, PadButton::DPadDown);
    focused_is(&h, at(body, &["music", "control"]), "the music slider");
    tap(&mut h, &mut m, PadButton::DPadDown);
    focused_is(
        &h,
        at(body, &["fullscreen", "control"]),
        "the fullscreen switch",
    );
    tap(&mut h, &mut m, PadButton::DPadRight);
    m.update(&mut h.ui);
    assert!(m.settings().fullscreen);
    assert!(matches!(m.take_events().as_slice(), [GameEvent::SettingsChanged(s)] if s.fullscreen));
    // East goes back to where settings were opened from.
    tap(&mut h, &mut m, PadButton::East);
    assert_eq!(m.screen(), Screen::Main);
    // Credits: the engine entry, first, as static text; Back returns.
    tap(&mut h, &mut m, PadButton::DPadDown);
    tap(&mut h, &mut m, PadButton::DPadDown);
    focused_is(&h, at(main, &["credits"]), "Credits");
    tap(&mut h, &mut m, PadButton::South);
    assert_eq!(m.screen(), Screen::Credits);
    let first_line = at(m.screen_id(Screen::Credits), &["list", "s0", "l0"]);
    let text =
        h.ui.widget::<Label>(first_line)
            .map(|l| l.text(h.ui.rt()))
            .unwrap_or_default();
    assert_eq!(text, ENGINE_CREDIT);
    assert!(h.ui.is_visible(first_line), "the credit is on screen");
    tap(&mut h, &mut m, PadButton::East);
    assert_eq!(m.screen(), Screen::Main);
    // Play: the HUD; Start pauses; East resumes.
    m.show(&mut h.ui, Screen::Main);
    h.settle();
    tap(&mut h, &mut m, PadButton::South);
    assert_eq!(m.screen(), Screen::Hud);
    tap(&mut h, &mut m, PadButton::Start);
    assert_eq!(m.screen(), Screen::Pause);
    assert!(
        h.ui.is_visible(m.screen_id(Screen::Hud)),
        "the HUD stays under the pause menu"
    );
    focused_is(&h, at(m.screen_id(Screen::Pause), &["resume"]), "Resume");
    tap(&mut h, &mut m, PadButton::East);
    assert_eq!(m.screen(), Screen::Hud);
    // Pause -> Settings -> Back returns to the pause menu, not the main menu.
    tap(&mut h, &mut m, PadButton::Start);
    tap(&mut h, &mut m, PadButton::DPadDown);
    tap(&mut h, &mut m, PadButton::South);
    assert_eq!(m.screen(), Screen::Settings);
    tap(&mut h, &mut m, PadButton::East);
    assert_eq!(m.screen(), Screen::Pause);
    // Quit to menu.
    tap(&mut h, &mut m, PadButton::DPadDown);
    tap(&mut h, &mut m, PadButton::DPadDown);
    tap(&mut h, &mut m, PadButton::South);
    assert_eq!(m.screen(), Screen::Main);
    let evs = m.take_events();
    assert_eq!(
        evs,
        vec![
            GameEvent::StartGame,
            GameEvent::Paused,
            GameEvent::Resume,
            GameEvent::Paused,
            GameEvent::QuitToMenu,
        ]
    );
}

#[test]
fn the_keyboard_drives_the_same_menu() {
    let (mut h, mut m) = setup();
    h.key(KeyCode::Down, Modifiers::NONE);
    h.key(KeyCode::Enter, Modifiers::NONE);
    let actions = h.ui.take_actions();
    m.on_actions(&mut h.ui, actions);
    h.settle();
    assert_eq!(m.screen(), Screen::Settings);
    // Escape reaches the key sink: back.
    h.key(KeyCode::Escape, Modifiers::NONE);
    let actions = h.ui.take_actions();
    m.on_actions(&mut h.ui, actions);
    assert_eq!(m.screen(), Screen::Main);
}

#[test]
fn a_held_direction_repeats_on_deadlines_and_an_idle_pad_schedules_nothing() {
    let (mut h, mut m) = setup();
    assert_eq!(m.next_deadline(), None, "no pad input: no deadline");
    let t0 = h.now();
    m.pad(
        &mut h.ui,
        t0,
        PadInput::Button {
            button: PadButton::DPadDown,
            pressed: true,
        },
    );
    h.settle();
    let main = m.screen_id(Screen::Main);
    focused_is(&h, at(main, &["settings"]), "one step on press");
    assert_eq!(m.next_deadline(), Some(t0 + REPEAT_DELAY));
    m.tick(&mut h.ui, t0 + REPEAT_DELAY - Duration::from_millis(1));
    focused_is(&h, at(main, &["settings"]), "no repeat before the delay");
    m.tick(&mut h.ui, t0 + REPEAT_DELAY);
    focused_is(&h, at(main, &["credits"]), "the first repeat");
    m.tick(&mut h.ui, t0 + REPEAT_DELAY + REPEAT_EVERY);
    focused_is(&h, at(main, &["quit"]), "then one per interval");
    m.pad(
        &mut h.ui,
        t0 + REPEAT_DELAY + REPEAT_EVERY,
        PadInput::Button {
            button: PadButton::DPadDown,
            pressed: false,
        },
    );
    assert_eq!(m.next_deadline(), None, "released: nothing scheduled");
    // The stick: past the press threshold moves once; resting under release stops.
    let t1 = h.now() + Duration::from_secs(1);
    m.pad(&mut h.ui, t1, PadInput::Stick { x: 0.0, y: 0.9 });
    h.settle();
    focused_is(&h, at(main, &["credits"]), "stick up");
    m.pad(&mut h.ui, t1, PadInput::Stick { x: 0.0, y: 0.45 });
    assert!(m.next_deadline().is_some(), "still held above the release");
    m.pad(&mut h.ui, t1, PadInput::Stick { x: 0.0, y: 0.1 });
    assert_eq!(m.next_deadline(), None);
}

#[test]
fn an_idle_menu_renders_nothing() {
    let (mut h, m) = setup();
    let frames = h.advance(Duration::from_secs(10));
    assert_eq!(frames, 0, "idle: no frames");
    assert_eq!(h.wakeups, 0, "idle: no wakeups");
    assert_eq!(m.next_deadline(), None);
}

/// The text a visible label or button shows (its accessible name).
fn visible_texts(h: &mut Harness, under: WidgetId) -> Vec<(WidgetId, String)> {
    let ids: Vec<WidgetId> = h.ui.walk().into_iter().map(|(id, _)| id).collect();
    let mut out = Vec::new();
    for id in ids {
        if !h.ui.is_visible(id) || !is_under(h, id, under) {
            continue;
        }
        if !matches!(h.ui.role(id), Some(Role::Label | Role::Button)) {
            continue;
        }
        if let Some(n) = h.node(id)
            && let Some(l) = n.label()
        {
            out.push((id, l.to_string()));
        }
    }
    out
}

fn is_under(h: &Harness, id: WidgetId, root: WidgetId) -> bool {
    let mut cur = Some(id);
    while let Some(c) = cur {
        if c == root {
            return true;
        }
        cur = h.ui.parent(c);
    }
    false
}

/// The visible strings of screen `s` that the pseudo-locale did not transform. Credit lines
/// are exempt: names and licences are not translated, and the engine entry is fixed (E-64).
fn unlocalised(h: &mut Harness, m: &mut GameMenu, s: Screen) -> Vec<(WidgetId, String)> {
    m.show(&mut h.ui, s);
    h.settle();
    let texts = visible_texts(h, m.screen_id(s));
    assert!(texts.len() >= 3, "{s:?}: {texts:?}");
    let lines: Vec<&str> = m.credits().lines().collect();
    texts
        .into_iter()
        .filter(|(_, t)| !is_pseudo(t) && !lines.contains(&t.as_str()))
        .collect()
}

#[test]
fn the_pseudo_locale_transforms_every_visible_menu_string() {
    let (mut h, mut m) = setup();
    assert!(m.set_language(&mut h.ui, "qps-ploc"));
    h.settle();
    for s in Screen::ALL {
        let raw = unlocalised(&mut h, &mut m, s);
        assert!(
            raw.is_empty(),
            "{s:?}: strings that were not looked up: {raw:?}"
        );
    }
    // The credits' headings are localised, their lines (names, licences, the engine entry)
    // shown as written.
    let heading = at(m.screen_id(Screen::Credits), &["list", "s0", "heading"]);
    let t = h.node(heading).and_then(|n| n.label().map(str::to_string));
    assert!(t.as_deref().is_some_and(is_pseudo), "{t:?}");
    // Every control's accessible name is in the new language too (the body was rebuilt).
    let body = at(m.screen_id(Screen::Settings), &["body"]);
    let slider = at(body, &["master", "control"]);
    let name = h.node(slider).and_then(|n| n.label().map(str::to_string));
    assert!(name.as_deref().is_some_and(is_pseudo), "{name:?}");
    // French, then back: labels follow; the prompt keeps the pad's glyphs.
    assert!(m.set_language(&mut h.ui, "fr"));
    h.settle();
    m.show(&mut h.ui, Screen::Main);
    h.settle();
    let play = at(m.screen_id(Screen::Main), &["play"]);
    assert_eq!(
        h.node(play).and_then(|n| n.label().map(str::to_string)),
        Some("Jouer".to_string())
    );
    let prompt = at(m.screen_id(Screen::Hud), &["prompt"]);
    let p =
        h.ui.widget::<Label>(prompt)
            .map(|l| l.text(h.ui.rt()))
            .unwrap_or_default();
    assert!(p.starts_with("A Choisir"), "{p}");
    m.set_glyphs(&mut h.ui, GlyphSet::PlayStation);
    let p =
        h.ui.widget::<Label>(prompt)
            .map(|l| l.text(h.ui.rt()))
            .unwrap_or_default();
    assert!(p.starts_with("\u{2715} Choisir"), "{p}");
    assert!(!m.set_language(&mut h.ui, "xx"));
}

#[test]
fn positive_control_a_hardcoded_string_is_caught_by_the_pseudo_audit() {
    // W2: the audit above must fail on a label that bypasses the localiser.
    let (mut h, mut m) = setup();
    let main = m.screen_id(Screen::Main);
    h.ui.add(
        main,
        "rogue",
        forge_ui::NodeStyle::leaf(),
        Label::new("Hard-coded"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(m.set_language(&mut h.ui, "qps-ploc"));
    h.settle();
    let texts = visible_texts(&mut h, main);
    assert!(
        texts
            .iter()
            .any(|(_, t)| t == "Hard-coded" && !is_pseudo(t)),
        "{texts:?}"
    );
}

#[test]
fn the_hud_follows_game_state_and_events_reach_the_world_as_messages() {
    let (mut h, mut m) = setup();
    tap(&mut h, &mut m, PadButton::South);
    assert_eq!(m.screen(), Screen::Hud);
    m.set_hud(&mut h.ui, 0.25, 1200);
    h.settle();
    let score = at(m.screen_id(Screen::Hud), &["top", "score"]);
    let s =
        h.ui.widget::<Label>(score)
            .map(|l| l.text(h.ui.rt()))
            .unwrap_or_default();
    assert_eq!(s, "Score: 1200");
    // An unchanged HUD value costs nothing.
    let before = h.frames;
    m.set_hud(&mut h.ui, 0.25, 1200);
    h.settle();
    assert_eq!(h.frames, before, "an unchanged HUD redraws nothing");
    let _ = Progress::Idle;
    let mut world = forge_core::bevy_ecs::world::World::new();
    tap(&mut h, &mut m, PadButton::Start);
    let n = m.deliver(&mut world);
    assert_eq!(n, 2, "StartGame and Paused");
    let mut q = world.resource_mut::<Messages<GameEvent>>();
    let got: Vec<GameEvent> = q.drain().collect();
    assert_eq!(got, vec![GameEvent::StartGame, GameEvent::Paused]);
}

#[test]
fn the_ui_scales_to_the_window() {
    let (_h, m) = setup();
    assert_eq!(m.scale_for(Size::new(1280.0, 720.0)), 1.0);
    assert_eq!(m.scale_for(Size::new(2560.0, 1440.0)), 2.0);
    assert_eq!(m.scale_for(Size::new(3840.0, 1600.0)), 1600.0 / 720.0);
}

#[test]
fn positive_control_an_untranslated_credits_heading_is_caught_by_the_pseudo_audit() {
    // W2: a credits section whose heading has no string shows it as written; the audit over
    // the credits screen must name it.
    let mut h = Harness::new(UiConfig {
        size: Size::new(1280.0, 720.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let mut cfg = GameUiConfig::default();
    cfg.credits = cfg.credits.section("Music", &["A composer"]);
    let mut m = GameMenu::build(&mut h.ui, cfg).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert!(m.set_language(&mut h.ui, "qps-ploc"));
    h.settle();
    let raw = unlocalised(&mut h, &mut m, Screen::Credits);
    assert!(
        raw.iter().any(|(_, t)| t == "Music"),
        "the audit missed an untranslated heading: {raw:?}"
    );
}

/// Messages held in `world` after `turns` turns of two events each.
fn held_after(turns: usize, advance: bool) -> usize {
    let mut world = forge_core::bevy_ecs::world::World::new();
    for _ in 0..turns {
        let evs = vec![GameEvent::Paused, GameEvent::Resume];
        if advance {
            forge_runtime::game_ui::end_turn(&mut world, evs);
        } else {
            forge_runtime::game_ui::deliver(&mut world, evs);
        }
    }
    world
        .get_resource::<Messages<GameEvent>>()
        .map_or(0, Messages::len)
}

#[test]
fn the_game_s_message_buffers_hold_two_turns_however_long_it_runs() {
    // Every turn advances the buffers: at most two turns of messages are ever held.
    let held = held_after(1000, true);
    assert!(
        held <= 4,
        "after 1000 turns the buffers hold {held} messages"
    );
    // The events of the last turn are still readable.
    assert!(held >= 2, "the last turn's events are gone: {held}");
}

#[test]
fn positive_control_delivering_without_advancing_grows_the_buffers() {
    // W2: without `Messages::update` every turn (the runtime's loop as found) nothing is
    // ever dropped.
    assert_eq!(held_after(1000, false), 2000);
}

#[test]
fn an_idle_menu_turn_allocates_nothing() {
    let (mut h, mut m) = setup();
    m.update(&mut h.ui);
    let idle = allocation_counter::measure(|| {
        for _ in 0..100 {
            m.update(&mut h.ui);
        }
    })
    .count_total;
    assert_eq!(
        idle, 0,
        "100 idle menu updates made {idle} heap allocations"
    );
    // The measure is live: a changed setting is applied (and allocates) on the next update.
    assert!(m.set_language(&mut h.ui, "fr"));
    let before = m.settings().language.clone();
    assert_eq!(before, "fr");
    let changed = allocation_counter::measure(|| {
        assert!(m.set_language(&mut h.ui, "en"));
    })
    .count_total;
    assert!(changed > 0, "a language switch measured no allocation");
}
