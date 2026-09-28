//! web3d-M4: `examples/survive3d` — the v1.0 slice — plays end to end.
//!
//! Drives the real game headless through the 3D render path (the same
//! `eval::render_frame3d` the native and web shells call each frame),
//! with scripted key presses, and checks the loop a player goes
//! through: fight, level up and pick an upgrade, pause and resume,
//! die, restart.

use twec::eval;
use twec::host3d::{apply_key_state, KEY_CODES};
use twec::render3d_types::HudItem;

const DT: f64 = 1.0 / 60.0;

struct Game {
    env: twec::value::Env,
}

impl Game {
    fn new() -> Self {
        // Assets resolve against the game's folder, as `twec play3d` sets.
        twec::bundle::set_asset_root(Some("examples/survive3d".into()));
        let src = std::fs::read_to_string("examples/survive3d/main.twe").expect("read game");
        let program = twec::parser::parse(&twec::lexer::lex(&src).expect("lex")).expect("parse");
        let mut env = twec::value::Env::new();
        twec::stdlib::install(&mut env);
        eval::run_top_level(&mut env, &program).expect("top level");
        Game { env }
    }

    /// One frame: input, a tick, a render. `held` / `pressed` are key names.
    fn frame(&mut self, held: &[&str], pressed: &[&str]) {
        let names: Vec<&str> = KEY_CODES.iter().map(|(n, _)| *n).collect();
        apply_key_state(&mut self.env, &names, &|n| held.contains(&n), &|n| {
            pressed.contains(&n)
        });
        eval::tick_frame(&mut self.env, DT).expect("tick");
        eval::render_frame3d(&mut self.env).expect("render");
    }

    fn state(&self) -> String {
        let scene = self
            .env
            .active_scene
            .as_ref()
            .expect("the game has a scene");
        scene.borrow().current_state.clone().unwrap_or_default()
    }

    fn global(&self, name: &str) -> f64 {
        let v = self
            .env
            .get(name)
            .unwrap_or_else(|| panic!("no global {name}"));
        if v.is_float() {
            v.as_float()
        } else {
            v.as_int() as f64
        }
    }

    fn hud_text(&self) -> String {
        self.env
            .hud_queue
            .iter()
            .filter_map(|h| match h {
                HudItem::Text { text, .. } => Some(text.as_str()),
                HudItem::Rect { .. } => None,
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

#[test]
fn survive3d_plays_through_a_run() {
    let mut g = Game::new();
    assert_eq!(g.state(), "playing");

    // Circle the arena so the auto-weapon has targets and gems get
    // collected; pick the first upgrade whenever the picker opens.
    let mut level_ups = 0;
    let mut frames = 0;
    while g.state() != "game_over" && frames < 60 * 60 * 5 {
        let dir = ["w", "d", "s", "a"][(frames / 90) % 4];
        if g.state() == "level_up" {
            assert!(g.hud_text().contains("LEVEL UP"), "{}", g.hud_text());
            g.frame(&[], &["1"]);
            level_ups += 1;
        } else {
            g.frame(&[dir], &[]);
        }
        if frames == 600 {
            // The HUD and world draw while playing.
            assert!(g.hud_text().contains("Wave 1"), "{}", g.hud_text());
            assert!(g.env.render_queue3d.len() > 20, "the world draws");
            // Pause and resume.
            g.frame(&[], &["escape"]);
            assert_eq!(g.state(), "paused");
            assert!(g.hud_text().contains("PAUSED"));
            g.frame(&[], &["escape"]);
            assert_eq!(g.state(), "playing");
        }
        frames += 1;
    }
    assert!(g.global("kills") > 10.0, "kills: {}", g.global("kills"));
    assert!(level_ups >= 1, "never levelled up");
    assert_eq!(
        g.state(),
        "game_over",
        "survived 5 minutes standing in the open?"
    );
    assert!(g.hud_text().contains("GAME OVER"), "{}", g.hud_text());

    // Restart gives a fresh run.
    g.frame(&[], &["r"]);
    assert_eq!(g.state(), "playing");
    assert_eq!(g.global("kills"), 0.0);
    assert_eq!(g.global("player_hp"), 100.0);
}

/// web3d-M4: in a 3D shell the game's sounds become audio commands
/// for the shell to play — shots from the start, hits as enemies die.
#[test]
fn survive3d_queues_its_sounds() {
    twec::audio_host::enable();
    let mut g = Game::new();
    twec::audio_host::drain();
    for _ in 0..60 * 20 {
        g.frame(&["w"], &[]);
    }
    let paths: Vec<String> = twec::audio_host::drain()
        .into_iter()
        .filter_map(|c| match c {
            twec::audio_host::AudioCmd::Play { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    assert!(paths.iter().any(|p| p == "assets/shot.wav"), "{paths:?}");
    assert!(paths.iter().any(|p| p == "assets/hit.wav"), "{paths:?}");
}
