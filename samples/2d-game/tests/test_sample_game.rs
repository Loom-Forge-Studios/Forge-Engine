//! `test_sample_game` — the 2D sample game is 2D's acceptance test (Ch.35 §35.3; DoD M4-11,
//! M4-12; Spike S13): driven by a gamepad, it clears its level, replays bit-identically,
//! shows every S13 feature on screen, and plays through its menus to the level-clear banner.
//!
//! * `the_scripted_run_clears_the_level_with_a_gamepad`: a `ScriptedPad` (the
//!   `GamepadSource` seam a platform pad backend fills) holds the stick right and taps South;
//!   the hero crosses the autotiled level, over its stone walls, collects coins and reaches
//!   the goal. Control: `positive_control_without_jumping_the_first_wall_stops_the_hero`.
//! * `the_run_replays_to_its_golden_bits`: the run's fingerprint (every body, contact
//!   impulse, particle and animation clock, folded every 30 steps) is
//!   [`SCRIPT_FINGERPRINT`] — the same on windows-x86_64 and on ubuntu-x86_64
//!   (`just verify-linux`, ADR 0044). Control:
//!   `positive_control_one_step_of_input_changes_the_bits`.
//! * `the_menus_play_the_level_to_the_clear_banner`: headless, the pad presses Play in the
//!   main menu, the scripted run plays through the game-UI runtime's session to the
//!   level-clear banner with the same fingerprint, the HUD's score follows the coins, and
//!   South returns to the main menu. Control:
//!   `positive_control_through_the_menus_without_jumping_there_is_no_banner`.
//! * `the_clock_runs_the_level_at_60_hz_and_a_pause_costs_nothing`, the credits' E-64 entry
//!   and every sample string translated.
//! * `every_s13_feature_is_on_screen`: a frame mid-run draws the tile map, the cutout rig's
//!   six parts, lights over the terrain's occluders, dust particles and the parallax strip,
//!   pixel-perfect at 4x (GPU; AWAITING without an adapter, a failure under `CI=true`).
use std::time::Duration;

use forge_2d_game::session::{Mode, Session, SessionConfig, credits, strings};
use forge_2d_game::{
    Fingerprint, Game, PadState, SCRIPT_FINGERPRINT, SCRIPT_STEPS, demo_script, play_script,
};
use forge_ui::game::{ENGINE_CREDIT, GamepadSource, GlyphSet, PadButton, PadInput, ScriptedPad};
use forge_ui::testing::Harness;
use forge_ui::{Size, UiConfig};

/// Play `script` through a scripted pad for `steps` fixed steps.
fn play(script: &[Vec<PadInput>], steps: usize) -> (Game, u64) {
    let mut game = Game::new().unwrap_or_else(|e| panic!("{e}"));
    let mut pad = ScriptedPad::new(GlyphSet::Generic);
    let mut state = PadState::default();
    let mut trace = Fingerprint::default();
    for i in 0..steps {
        if let Some(inputs) = script.get(i) {
            for x in inputs {
                pad.push(*x);
            }
        }
        for x in pad.poll() {
            state.apply(x);
        }
        game.step(&state).unwrap_or_else(|e| panic!("{e}"));
        trace.observe(i as u64, &game);
    }
    (game, trace.0)
}

/// The shipped game's session in a headless UI, at the main menu.
fn session() -> (Harness, Session) {
    let mut h = Harness::new(UiConfig {
        size: Size::new(1280.0, 720.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let s = Session::build(&mut h.ui, &SessionConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    (h, s)
}

fn tap(h: &mut Harness, s: &mut Session, b: PadButton) {
    let now = h.now();
    for pressed in [true, false] {
        s.pad(&mut h.ui, now, PadInput::Button { button: b, pressed });
    }
    h.settle();
}

/// Through the menus: Play, then `script` step by step until the level ends or `steps` ran.
fn play_through_menus(script: Vec<Vec<PadInput>>, steps: usize) -> (Harness, Session) {
    let (mut h, mut s) = session();
    assert_eq!(s.mode(), Mode::Menu);
    tap(&mut h, &mut s, PadButton::South);
    assert_eq!(s.mode(), Mode::Playing, "South on Play starts the level");
    s.set_script(script);
    for _ in 0..steps {
        s.step_once(&mut h.ui);
        if s.mode() != Mode::Playing {
            break;
        }
    }
    h.settle();
    (h, s)
}

#[test]
fn the_scripted_run_clears_the_level_with_a_gamepad() {
    let (g, _) = play(&demo_script(900), 900);
    let hero = g.hero().unwrap_or_else(|e| panic!("{e}"));
    println!(
        "after 900 steps: hero at {:?}, {} coins, won {}",
        hero.local, g.coins_taken, g.won
    );
    assert!(g.won, "the hero reaches the goal: {:?}", hero.local);
    assert!(g.coins_taken >= 4, "{} coins", g.coins_taken);
    assert!(
        hero.local.y > -10.0,
        "the hero is on the level, not falling: {:?}",
        hero.local
    );
}

#[test]
fn positive_control_without_jumping_the_first_wall_stops_the_hero() {
    let script: Vec<Vec<PadInput>> = (0..900)
        .map(|i| {
            if i == 0 {
                vec![PadInput::Stick { x: 1.0, y: 0.0 }]
            } else {
                Vec::new()
            }
        })
        .collect();
    let (g, _) = play(&script, 900);
    let hero = g.hero().unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !g.won && hero.local.x < 10.0,
        "no jump, no way past the stone wall: {:?}",
        hero.local
    );
}

#[test]
fn the_run_replays_to_its_golden_bits() {
    let (a, fa) = play(&demo_script(900), 900);
    let (b, fb) = play(&demo_script(900), 900);
    assert_eq!(a.state_bits(), b.state_bits(), "a replay is bit-identical");
    assert_eq!(fa, fb);
    let f = fa;
    println!(
        "sample game run fingerprint on {}-{}: {f:#018x}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    assert_eq!(
        Fingerprint(f),
        SCRIPT_FINGERPRINT,
        "the run's golden fingerprint (update SCRIPT_FINGERPRINT only with a reason)"
    );
    // The library's loop (the corpus row's and the exported binary's) is the same run.
    let mut lib = Fingerprint::default();
    play_script(&demo_script(900), 900, |i, g| lib.observe(i, g)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(lib, SCRIPT_FINGERPRINT);
}

#[test]
fn the_menus_play_the_level_to_the_clear_banner() {
    let (mut h, mut s) = play_through_menus(demo_script(SCRIPT_STEPS as u64), SCRIPT_STEPS);
    assert_eq!(s.mode(), Mode::Won, "the level is clear");
    assert!(h.ui.is_visible(s.banner()), "the level-clear banner shows");
    let g = s.game();
    let (clear, prompt) = s.banner_texts(&h.ui);
    assert_eq!(clear, format!("Level clear! {} coins", g.coins_taken));
    assert!(prompt.contains("Main menu"), "{prompt}");
    println!(
        "through the menus: won after {} steps, {} coins, fingerprint {:#018x}",
        g.steps,
        g.coins_taken,
        s.fingerprint().0
    );
    // The level was cleared before the script ran out: its fingerprint is the core run's
    // folded up to that step.
    let mut core = Fingerprint::default();
    let steps = g.steps as usize;
    play_script(&demo_script(SCRIPT_STEPS as u64), steps, |i, g| {
        core.observe(i, g)
    })
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        s.fingerprint(),
        core,
        "the menus' session plays the same bits as the core"
    );
    assert_eq!(
        (g.steps, s.fingerprint()),
        forge_2d_game::SCRIPT_WIN,
        "the pinned win state (what every build's --play-script prints)"
    );
    assert_eq!(
        s.log(),
        [forge_runtime::game_ui::GameEvent::StartGame],
        "Play reached the game as the StartGame message"
    );
    tap(&mut h, &mut s, PadButton::South);
    assert_eq!(
        s.mode(),
        Mode::Menu,
        "South on the banner returns to the menu"
    );
    assert!(!h.ui.is_visible(s.banner()));
    assert!(h.ui.is_visible(s.menu().screen_id(forge_runtime::game_ui::Screen::Main)));
}

#[test]
fn positive_control_through_the_menus_without_jumping_there_is_no_banner() {
    let script = vec![vec![PadInput::Stick { x: 1.0, y: 0.0 }]];
    let (h, s) = play_through_menus(script, SCRIPT_STEPS);
    assert_eq!(s.mode(), Mode::Playing);
    assert!(!h.ui.is_visible(s.banner()));
}

#[test]
fn the_clock_runs_the_level_at_60_hz_and_a_pause_costs_nothing() {
    let (mut h, mut s) = session();
    tap(&mut h, &mut s, PadButton::South);
    let t0 = h.now();
    // A 60 Hz display: one turn per frame for one second.
    for f in 1..=60u32 {
        let now = t0 + Duration::from_micros(16_667 * u64::from(f));
        s.advance(&mut h.ui, now);
    }
    assert_eq!(s.game().steps, 60, "one second is 60 fixed steps");
    let due = s
        .next_deadline()
        .unwrap_or_else(|| panic!("a playing level has a deadline"));
    assert!(due > t0 + Duration::from_secs(1));
    // A stall longer than 15 steps drops the backlog instead of fast-forwarding.
    s.advance(&mut h.ui, t0 + Duration::from_secs(5));
    assert_eq!(s.game().steps, 60);
    // Start pauses: no deadline, no step.
    tap(&mut h, &mut s, PadButton::Start);
    assert_eq!(s.mode(), Mode::Paused);
    assert_eq!(s.next_deadline(), None, "a paused game schedules nothing");
    assert_eq!(s.advance(&mut h.ui, t0 + Duration::from_secs(9)), 0);
    // East resumes from the pause menu, on a fresh cadence.
    tap(&mut h, &mut s, PadButton::East);
    assert_eq!(s.mode(), Mode::Playing);
}

#[test]
fn the_credits_carry_the_engine_entry_and_every_string_is_translated() {
    let (_h, s) = session();
    let shown = s.menu().credits();
    assert!(shown.has_engine_entry());
    assert_eq!(
        shown.lines().next(),
        Some(ENGINE_CREDIT),
        "E-64: the engine entry comes first"
    );
    assert_eq!(shown.sections().len(), credits().sections().len());
    let mut loc = strings();
    for key in [
        "menu.title",
        "hud.prompt",
        "game2d.window_title",
        "game2d.view",
        "game2d.clear",
        "game2d.clear_prompt",
        "credits.section.Level",
        "credits.section.Fonts",
    ] {
        let en = loc
            .raw("en", key)
            .unwrap_or_else(|| panic!("{key} has no English string"));
        let fr = loc
            .raw("fr", key)
            .unwrap_or_else(|| panic!("{key} has no French string"));
        assert_ne!(en, fr, "{key} is translated");
    }
    assert!(loc.set_current(forge_ui::l10n::PSEUDO_LOCALE));
    let pseudo = loc.get("game2d.clear", &[("coins", "3")]);
    assert!(forge_ui::l10n::is_pseudo(&pseudo), "{pseudo}");
}

#[test]
fn positive_control_one_step_of_input_changes_the_bits() {
    let mut script = demo_script(900);
    // Let go of the stick for one step, once.
    script[100].push(PadInput::Stick { x: 0.0, y: 0.0 });
    script[101].push(PadInput::Stick { x: 1.0, y: 0.0 });
    let (_, fa) = play(&demo_script(900), 900);
    let (_, fb) = play(&script, 900);
    assert_ne!(fa, fb);
}

#[test]
fn every_s13_feature_is_on_screen() {
    use forge_2d::render::{RenderOptions2d, Renderer2d};
    use forge_gpu::{AdapterPool, PoolOptions, wgpu};
    let (g, _) = play(&demo_script(300), 300);
    let pool = match AdapterPool::new(&PoolOptions::default()) {
        Ok(p) => p,
        Err(e) => {
            let ci = std::env::var("CI").is_ok_and(|v| v == "true" || v == "1");
            assert!(!ci, "CI=true and no GPU adapter: {e}");
            println!("C-2d-sample-game: AWAITING(no GPU adapter: {e})");
            return;
        }
    };
    let dev = pool.primary();
    let (w, h) = (1280, 720);
    let mut r = Renderer2d::new(dev, RenderOptions2d::new(w, h)).unwrap_or_else(|e| panic!("{e}"));
    let (t, atlas) = forge_2d::scenes::upload(&mut r, dev).unwrap_or_else(|e| panic!("{e}"));
    let f = g.frame(&t, &atlas).unwrap_or_else(|e| panic!("{e}"));
    let tiles = f.sprites.iter().filter(|s| s.texture == t.tiles).count();
    let cutout = f
        .sprites
        .iter()
        .filter(|s| s.texture == t.white && s.layer == 5)
        .count();
    let dust = f
        .sprites
        .iter()
        .filter(|s| s.texture == t.white && s.layer == 6)
        .count();
    assert!(tiles > 30, "the tile map is drawn: {tiles} tiles");
    assert_eq!(cutout, 6, "the cutout rig's six parts");
    assert!(
        f.lights.len() >= 2 && !f.occluders.is_empty(),
        "lights over the terrain's occluders"
    );
    assert!(f.parallax.contains_key(&-10), "the parallax strip");
    println!(
        "frame: {tiles} tiles, {cutout} rig parts, {dust} dust, {} lights",
        f.lights.len()
    );
    let target = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("platformer frame"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let rep = r.render(dev, &f, &target).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((rep.render_size, rep.scale), ((320, 180), 4));
    let px = forge_gpu::transfer::read_texture(dev, &target).unwrap_or_else(|e| panic!("{e}"));
    let at = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    // Pixel-perfect: 4x4 blocks of one colour.
    for (bx, by) in [(0, 0), (640, 360), (1276, 716), (400, 600)] {
        let p = at(bx, by);
        assert!((0..4).all(|y| (0..4).all(|x| at(bx + x, by + y) == p)));
    }
    let distinct: std::collections::BTreeSet<[u8; 4]> =
        px.as_chunks::<4>().0.iter().copied().step_by(97).collect();
    assert!(
        distinct.len() > 50,
        "a lit, textured frame: {} colours",
        distinct.len()
    );
}
