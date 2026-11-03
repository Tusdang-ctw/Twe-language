//! Lanternfall: the battle royale plays, headless.
//!
//! Loads the multi-file game the way `twec play3d` does
//! (`module::prepare_entry`), then drives it through `host3d::InputState`
//! and `host3d::sim_tick` like the shells, with scripted keys and a
//! cursor, and reads the state back with probes.

use twec::host3d::{sim_tick, InputState, KEY_CODES};

const DT: f64 = 1.0 / 60.0;
const MAIN: &str = "examples/lanternfall/main.twe";

struct Game {
    env: twec::value::Env,
    input: InputState,
}

impl Game {
    fn new() -> Self {
        twec::bundle::set_asset_root(Some("examples/lanternfall".into()));
        let src = std::fs::read_to_string(MAIN).expect("read main.twe");
        let env = twec::module::prepare_entry(std::path::Path::new(MAIN), &src).expect("load the game");
        let mut input = InputState::default();
        input.set_view(1280, 720);
        // The cursor straight ahead of the keeper's starting spot.
        input.pointer(0.5, 0.35);
        Game { env, input }
    }

    /// `frames` frames with `held` keys down and `pressed` keys pressed
    /// on the first one.
    fn run(&mut self, frames: u32, held: &[&str], pressed: &[&str]) {
        for f in 0..frames {
            for (name, _) in KEY_CODES {
                if f == 0 && pressed.contains(name) {
                    self.input.key_up(name);
                    self.input.key_down(name);
                } else if held.contains(name) {
                    self.input.key_down(name);
                } else {
                    self.input.key_up(name);
                }
            }
            sim_tick(&mut self.env, &mut self.input, DT).expect("tick");
            twec::eval::render_frame3d(&mut self.env).expect("render");
        }
    }

    fn probe(&mut self, expr: &str) -> String {
        let src = format!("print({expr})\n");
        let program = twec::parser::parse(&twec::lexer::lex(&src).expect("lex")).expect("parse");
        self.env.out.clear();
        twec::eval::run_top_level(&mut self.env, &program).expect("probe");
        let out = self.env.out.trim().to_string();
        self.env.out.clear();
        out
    }

    fn num(&mut self, expr: &str) -> f64 {
        let s = self.probe(expr);
        s.parse().unwrap_or_else(|_| panic!("{expr} = {s}"))
    }
}

#[test]
fn the_island_loads_and_runs() {
    let mut g = Game::new();
    // The island is populated: blocks, lanterns, props.
    assert!(g.num("entities.count(world.Block)") >= 30.0);
    assert!(g.num("entities.count(world.LanternPost)") >= 20.0);
    assert!(g.num("entities.count(world.Tree) + entities.count(world.Rock)") >= 40.0);
    // Ten seconds of standing still: no error, and the keeper is on the
    // ground (not fallen through it).
    g.run(600, &[], &[]);
    let y = g.num("me.pos.y");
    assert!(y.abs() < 0.2, "the keeper stands on the ground: y = {y}");
}

#[test]
fn the_keeper_walks_and_walls_stop_it() {
    let mut g = Game::new();
    g.run(30, &[], &[]);
    let z0 = g.num("me.pos.z");
    // W walks north (-z) at walking speed.
    g.run(60, &["w"], &[]);
    let z1 = g.num("me.pos.z");
    assert!(z0 - z1 > 4.0 && z0 - z1 < 6.0, "walked {} m in a second", z0 - z1);
    // The vault's south wall (z = 11, a gate in its middle): walk west of
    // the gate, then north into the wall for three seconds.
    let wall_z = 11.0 + 0.6 + 0.34; // wall half-thickness + capsule radius
    g.run(60, &["a"], &[]);
    g.run(180, &["w"], &[]);
    let x = g.num("me.pos.x");
    let z = g.num("me.pos.z");
    assert!(x < -3.5 && x > -10.5, "walked west of the gate: x = {x}");
    assert!(z > wall_z - 0.1, "the wall stops the keeper: z = {z}");
}

#[test]
fn the_keeper_faces_the_cursor_and_rolls() {
    let mut g = Game::new();
    g.run(20, &[], &[]);
    // The cursor is ahead (north, -z) of the keeper: it faces about pi.
    let h = g.num("me.heading");
    assert!((h.abs() - std::f64::consts::PI).abs() < 0.5, "faces the cursor: heading {h}");
    // A roll covers ground fast, then cools down.
    let z0 = g.num("me.pos.z");
    g.run(1, &["w"], &["shift"]);
    g.run(15, &["w"], &[]);
    let z1 = g.num("me.pos.z");
    assert!(z0 - z1 > 3.0, "rolled {} m in a quarter second", z0 - z1);
    assert!(g.num("me.dodge_cd") > 0.5, "the roll cools down");
}
