//! web3d-M4: `examples/survive3d` — the v1.0 slice — plays end to end.
//!
//! Drives the real game headless through the 3D render path (the same
//! `eval::render_frame3d` the native and web shells call each frame),
//! with scripted key presses, and checks the loop a player goes
//! through: fight, level up and pick an upgrade, pause and resume,
//! die, restart. Input goes through `host3d::InputState` and
//! `host3d::sim_tick`, as in the shells, so a recorded run replays.

use twec::eval;
use twec::host3d::{sim_tick, InputState, KEY_CODES};
use twec::render3d_types::HudItem;

const DT: f64 = 1.0 / 60.0;

fn data_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("twe-survive3d-{}", std::process::id()))
}

struct Game {
    env: twec::value::Env,
    input: InputState,
}

impl Game {
    fn new() -> Self {
        // Assets resolve against the game's folder, as `twec play3d` sets.
        twec::bundle::set_asset_root(Some("examples/survive3d".into()));
        // The best run is saved under `os.data_dir`: keep it in target/.
        std::env::set_var("TWE_DATA_DIR", data_dir());
        let src = std::fs::read_to_string("examples/survive3d/main.twe").expect("read game");
        let program = twec::parser::parse(&twec::lexer::lex(&src).expect("lex")).expect("parse");
        let mut env = twec::value::Env::new();
        twec::stdlib::install(&mut env);
        eval::run_top_level(&mut env, &program).expect("top level");
        Game {
            env,
            input: InputState::default(),
        }
    }

    /// One frame: input, a tick, a render. `held` keys are down for the
    /// frame; `pressed` keys go down this frame (and are released).
    fn frame(&mut self, held: &[&str], pressed: &[&str]) {
        for (name, _) in KEY_CODES {
            if pressed.contains(name) {
                self.input.key_up(name);
                self.input.key_down(name);
            } else if held.contains(name) {
                self.input.key_down(name);
            } else {
                self.input.key_up(name);
            }
        }
        sim_tick(&mut self.env, &mut self.input, DT).expect("tick");
        eval::render_frame3d(&mut self.env).expect("render");
        for name in pressed {
            self.input.key_up(name);
        }
    }

    /// Move the mouse to canvas point (x, y) and click there.
    fn click(&mut self, x: f64, y: f64) {
        self.input.mouse_move(x, y);
        self.input.mouse_motion(1.0, 0.0);
        self.input.button_down("left");
        self.frame(&[], &[]);
        self.input.button_up("left");
    }

    /// A line summarising the run so far, for comparing two runs.
    fn fingerprint(&mut self) -> String {
        let probe = "print(\"{player.x} {player.z} {player_hp} {kills} {xp} {level} {entities.count(Slime)}\")";
        let program = twec::parser::parse(&twec::lexer::lex(probe).expect("lex")).expect("parse");
        self.env.out.clear();
        eval::run_top_level(&mut self.env, &program).expect("probe");
        let line = format!("{} {}", self.state(), self.env.out.trim());
        self.env.out.clear();
        line
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
            // Alternate: a key, then a click on the second card.
            if level_ups % 2 == 0 {
                g.frame(&[], &["1"]);
            } else {
                g.click(320.0, 238.0);
            }
            assert_eq!(g.state(), "playing", "the pick closes the picker");
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
    assert!(level_ups >= 2, "levelled up {level_ups} times");
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

    // The best run was saved, and a new session reads it back.
    let best = data_dir().join("survive3d").join("best.json");
    assert!(best.exists(), "no save at {}", best.display());
    let (wave, time) = (g.global("best_wave"), g.global("best_time"));
    assert!(time > 10.0, "best time {time}");
    let again = Game::new();
    assert_eq!(again.global("best_wave"), wave);
    assert!((again.global("best_time") - time).abs() < 1e-3);
    let _ = std::fs::remove_dir_all(data_dir());
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

/// web3d-M4: a run recorded through the input-command stream plays
/// back identically in a fresh game fed no live input: input is the
/// only thing that enters the simulation.
#[test]
fn survive3d_replays_a_recorded_run() {
    let log = std::env::temp_dir().join(format!("twe-survive3d-{}.replay", std::process::id()));
    let log = log.to_str().expect("utf-8 temp path").to_string();
    const TICKS: usize = 60 * 45;

    let mut live = Game::new();
    twec::replay::start_recording(&log).expect("record");
    let mut want = Vec::new();
    for t in 0..TICKS {
        if live.state() == "level_up" {
            live.frame(&[], &["2"]);
        } else {
            let dir = ["w", "d", "s", "a"][(t / 70) % 4];
            let also = if t % 200 < 40 { "d" } else { dir };
            live.frame(&[dir, also], &[]);
        }
        if t % 60 == 0 {
            want.push(live.fingerprint());
        }
    }
    twec::replay::stop();
    assert!(
        live.fingerprint().split(' ').nth(4).is_some_and(|k| k != "0"),
        "the recorded run should fight: {}",
        live.fingerprint()
    );

    let mut replayed = Game::new();
    twec::replay::start_playing(&log).expect("play");
    let mut got = Vec::new();
    for t in 0..TICKS {
        replayed.frame(&[], &[]);
        if t % 60 == 0 {
            got.push(replayed.fingerprint());
        }
    }
    twec::replay::stop();
    let _ = std::fs::remove_file(&log);
    for (i, (w, g)) in want.iter().zip(&got).enumerate() {
        assert_eq!(w, g, "diverged by second {i}");
    }
}

/// web3d-M4: the left stick moves the player; Start pauses.
#[test]
fn survive3d_plays_with_a_gamepad() {
    let mut g = Game::new();
    let mut buttons = [false; 14];
    for _ in 0..60 {
        g.input.set_gamepad(Some((&buttons, [1.0, 0.0, 0.0, 0.0, 0.0, 0.0])));
        g.frame(&[], &[]);
    }
    let x = g.fingerprint();
    let px: f64 = x.split(' ').nth(1).unwrap().parse().unwrap();
    assert!(px > 4.0, "stick right for 1 s: {x}");

    buttons[8] = true; // start
    g.input.set_gamepad(Some((&buttons, [0.0; 6])));
    g.frame(&[], &[]);
    assert_eq!(g.state(), "paused");
    buttons[8] = false;
    g.input.set_gamepad(Some((&buttons, [0.0; 6])));
    g.frame(&[], &[]);
    buttons[8] = true;
    g.input.set_gamepad(Some((&buttons, [0.0; 6])));
    g.frame(&[], &[]);
    assert_eq!(g.state(), "playing");
}
