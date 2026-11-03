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
    /// The game as it boots: on the title screen.
    fn title() -> Self {
        // Each game saves its settings and record into its own fresh
        // folder, never the player's real one.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let data = std::path::PathBuf::from(format!("target/lanternfall/data/{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::env::set_var("TWE_DATA_DIR", &data);
        twec::bundle::set_asset_root(Some("examples/lanternfall".into()));
        let src = std::fs::read_to_string(MAIN).expect("read main.twe");
        let env = twec::module::prepare_entry(std::path::Path::new(MAIN), &src).expect("load the game");
        let mut input = InputState::default();
        input.set_view(1280, 720);
        // The cursor straight ahead of the keeper's starting spot.
        input.pointer(0.5, 0.35);
        Game { env, input }
    }

    /// A round, just started (Play chosen on the title).
    fn new() -> Self {
        let mut g = Game::title();
        g.exec("act_on(\"play\")");
        g
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

    /// Run statements in the game's world (a test's setup).
    fn exec(&mut self, src: &str) {
        let src = format!("{src}
");
        let program = twec::parser::parse(&twec::lexer::lex(&src).expect("lex")).expect("parse");
        twec::eval::run_top_level(&mut self.env, &program).expect("exec");
    }

    /// Clear the rivals off the island, for tests of the keeper alone.
    fn no_rivals(&mut self) {
        self.exec("for b in entities.of(bots.Bot):
    if b.body >= 0:
        physics.despawn(b.body)
    if b.held != nil:
        b.held.gone = true
    despawn b");
        self.run(1, &[], &[]);
    }

    /// The training yard: no round, no rivals; the keeper on the ground
    /// south of the vault (0, 0, 24) with a pistol, five dummies further
    /// south (z = 34), the middle one unshielded.
    fn practice(&mut self) {
        self.exec(concat!(
            "stage = \"practice\"
",
            "if barge != nil:
    barge.gone()
    barge = nil
",
            "me.phase = \"ground\"
me.pos = vec3(0, 0, 24)
cam_x = 0.0
cam_z = 24.0
cam_h = tuning.CAM_HEIGHT
",
            "me.guns = [0, 1, -1]
me.mag = [0, 12, 0]
me.reserve = 60
me.slot = 1
me.cells = 2
",
            "for i in 0..<5:
    let d = spawn combat.Dummy at vec3(-8.0 + i * 4.0, 0, 34.0)
    d.turn = 3.14
    if i == 2:
        d.shield = 0.0
",
        ));
        self.no_rivals();
        self.exec("var everyone = [d for d in entities.of(combat.Dummy)]
everyone.append(me)
combat.actors = everyone");
    }

    fn num(&mut self, expr: &str) -> f64 {
        let s = self.probe(expr);
        s.parse().unwrap_or_else(|_| panic!("{expr} = {s}"))
    }
}

#[test]
fn the_island_loads_and_runs() {
    let mut g = Game::new();
    g.practice();
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
    g.practice();
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
    g.practice();
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

impl Game {
    /// Put the cursor at fraction (fx, fy) of the view.
    fn point(&mut self, fx: f64, fy: f64) {
        self.input.pointer(fx, fy);
    }

    fn click_frames(&mut self, frames: u32) {
        self.input.button_down("left");
        self.run(frames, &[], &[]);
        self.input.button_up("left");
    }
}

#[test]
fn the_pistol_hits_a_dummy_and_runs_dry() {
    let mut g = Game::new();
    g.practice();
    // The dummies stand south of the keeper (+z, toward the camera): aim
    // straight down the screen at the middle one, which has no shield.
    g.point(0.5, 0.9);
    g.run(20, &[], &[]);
    let mag0 = g.num("me.mag[1]");
    g.click_frames(70);
    let hp = g.num("entities.of(combat.Dummy)[2].hp");
    let mag1 = g.num("me.mag[1]");
    assert!(hp < 100.0, "the middle dummy was hit: hp {hp}");
    assert!(mag1 < mag0, "rounds spent: {mag0} -> {mag1}");
    // Empty the magazine: firing on empty starts a reload.
    g.click_frames(200);
    g.run(90, &[], &[]);
    // R tops it up from the reserve.
    g.run(1, &[], &["r"]);
    g.run(90, &[], &[]);
    assert_eq!(g.num("me.mag[1]"), 12.0, "reloaded");
    assert!(g.num("me.reserve") < 60.0, "from the reserve");
    // The dummy went down at some point and got back up within seconds.
    assert!(g.num("me.kills") >= 1.0, "a kill counted");
}

#[test]
fn shields_soak_first_and_crack() {
    let mut g = Game::new();
    g.practice();
    g.run(5, &[], &[]);
    g.probe("combat.hurt(entities.of(combat.Dummy)[0], 30.0, me, vec3(0, 0, 0))");
    assert_eq!(g.num("entities.of(combat.Dummy)[0].shield"), 20.0);
    assert_eq!(g.num("entities.of(combat.Dummy)[0].hp"), 100.0);
    g.probe("combat.hurt(entities.of(combat.Dummy)[0], 30.0, me, vec3(0, 0, 0))");
    assert_eq!(g.num("entities.of(combat.Dummy)[0].shield"), 0.0);
    assert_eq!(g.num("entities.of(combat.Dummy)[0].hp"), 90.0);
    assert!(g.num("entities.count(combat.Popup)") >= 1.0, "CRACKED pops up");
}

#[test]
fn a_mortar_shell_lands_and_bursts() {
    let mut g = Game::new();
    g.practice();
    g.run(5, &[], &[]);
    // Lob one at the middle dummies from the keeper.
    g.probe("combat.lob(me, me.pos + vec3(0, 1, 0), vec3(0, 0, 34), 64.0)");
    assert_eq!(g.num("entities.count(combat.Shell)"), 1.0);
    g.run(90, &[], &[]);
    assert_eq!(g.num("entities.count(combat.Shell)"), 0.0, "the shell landed");
    let hp = g.num("entities.of(combat.Dummy)[2].hp");
    assert!(hp < 100.0, "the burst hurt the dummy under it: hp {hp}");
}

#[test]
fn starcall_falls_where_you_point() {
    let mut g = Game::new();
    g.practice();
    g.point(0.5, 0.9);
    g.run(10, &[], &[]);
    g.run(1, &[], &["q"]);
    assert_eq!(g.num("entities.count(combat.Star)"), 1.0);
    assert!(g.num("me.star_cd") > 17.0, "on cooldown");
    g.run(90, &[], &[]);
    assert_eq!(g.num("entities.count(combat.Star)"), 0.0, "the star landed");
}

#[test]
fn holding_f_turns_a_cell_into_shield() {
    let mut g = Game::new();
    g.practice();
    g.run(10, &[], &[]);
    g.run(170, &["f"], &[]);
    assert_eq!(g.num("me.shield"), 25.0);
    assert_eq!(g.num("me.cells"), 1.0);
}

/// Render a firefight headless and save it, for eyes-on review
/// (`target/lanternfall/firefight.png`). Skips without a GPU.
#[test]
fn firefight_renders() {
    let mut renderer = match pollster::block_on(twec::kernel::render::Renderer::new_headless(960, 540)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let mut g = Game::new();
    g.practice();
    g.input.set_view(960, 540);
    g.point(0.5, 0.92);
    g.run(30, &[], &[]);
    // A rival beside the keeper, to see its name tag and health pips.
    g.exec("let near = spawn bots.Bot at vec3(5, 0, 27)
near.name = \"MossyWick42\"
near.shield = 50.0
near.model = \"assets/folk_ember.glb\"");
    g.probe("combat.lob(me, me.pos + vec3(0, 1, 0), vec3(4, 0, 34), 64.0)");
    g.run(40, &[], &[]);
    g.input.button_down("left");
    g.run(3, &[], &[]);
    let mut assets = twec::play3d::NativeAssets::default();
    for _ in 0..200 {
        twec::host3d::render_frame(&mut renderer, &mut g.env, &mut assets).expect("render");
        if assets.pending() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    g.run(1, &[], &[]);
    twec::host3d::render_frame(&mut renderer, &mut g.env, &mut assets).expect("render");
    let rgba = renderer.read_pixels().expect("pixels");
    std::fs::create_dir_all("target/lanternfall").unwrap();
    image::save_buffer("target/lanternfall/firefight.png", &rgba, 960, 540, image::ColorType::Rgba8).unwrap();
}

#[test]
fn the_rivals_fight_each_other() {
    let mut g = Game::new();
    let n = g.num("entities.count(bots.Bot)");
    assert_eq!(n, 31.0, "31 rivals on the island");
    let name = g.probe("entities.of(bots.Bot)[0].name");
    assert!(name.len() > 6 && name.chars().last().unwrap().is_ascii_digit(), "a handle: {name}");
    // Keep the keeper out of it: unhittable, out of the way.
    g.exec("me.alive = false");
    // Three simulated minutes: bots meet, fight and fall.
    let start = std::time::Instant::now();
    g.run(3 * 60 * 60, &[], &[]);
    let per_tick = start.elapsed().as_secs_f64() * 1000.0 / (3.0 * 3600.0);
    let alive = g.num("[b for b in entities.of(bots.Bot) if b.alive].length");
    let feed = g.num("combat.feed.length");
    eprintln!("after 3 min: {alive} bots alive, {feed} kills; {per_tick:.3} ms per tick (tick + render queue; debug builds are ~30x slower)");
    assert!(feed >= 3.0, "the rivals killed each other: {feed} kills");
    assert!(alive < 31.0, "some fell: {alive} alive");
}

// --- Lanternfall G4: the round ---

#[test]
fn a_round_starts_aboard_and_the_keeper_drops_in() {
    let mut g = Game::new();
    assert_eq!(g.probe("stage"), "barge");
    assert_eq!(g.probe("me.phase"), "aboard");
    assert_eq!(g.num("entities.count(bots.Bot)"), 31.0);
    assert!(g.num("entities.count(loot.Chest)") >= 50.0, "chests are out");
    // Ride a while, then leap: the keeper glides down and lands.
    g.run(120, &[], &[]);
    g.run(1, &[], &["space"]);
    assert_eq!(g.probe("me.phase"), "glide");
    g.run(60 * 9, &["w"], &[]);
    assert_eq!(g.probe("me.phase"), "ground", "landed: at {}", g.probe("me.pos"));
    assert!(g.num("me.pos.y").abs() < 0.3);
    // Once everyone is down the round is on and the Hush starts.
    g.run(60 * 25, &[], &[]);
    assert_eq!(g.probe("stage == \"playing\" or stage == \"over\""), "true");
    assert_eq!(g.probe("hush.running"), "true");
}

#[test]
fn a_chest_spills_loot_the_keeper_can_take() {
    let mut g = Game::new();
    g.practice();
    g.exec("me.guns = [0, -1, -1]\nme.slot = 0\nlet c = spawn loot.Chest at me.pos + vec3(1.2, 0, 0)\nc.tier = 3");
    g.run(5, &[], &[]);
    assert_eq!(g.probe("me.prompt"), "E  open chest", "at {} with {} loot near", g.probe("me.pos"), g.probe("entities.count(loot.Loot)"));
    g.run(1, &[], &["e"]);
    assert_eq!(g.num("[c for c in entities.of(loot.Chest) if c.opened].length"), 1.0, "the chest opened");
    g.run(60, &[], &[]);
    assert!(g.num("entities.count(loot.Loot)") >= 2.0, "it spilled loot");
    // Walk around the chest until the weapon's in reach, then take it.
    let mut took = false;
    for _ in 0..40 {
        if g.probe("me.prompt").starts_with("E  take") {
            g.run(1, &[], &["e"]);
            took = true;
            break;
        }
        g.exec("for l in entities.of(loot.Loot):\n    if l.kind == loot.WEAPON:\n        me.aim = l.pos");
        g.run(4, &["d"], &[]);
    }
    if !took {
        // Spilled out of the keeper's path: put it at its feet.
        g.exec("for l in entities.of(loot.Loot):\n    if l.kind == loot.WEAPON:\n        l.pos = vec3(me.pos.x, 0.35, me.pos.z)");
        g.run(2, &[], &[]);
        g.run(1, &[], &["e"]);
    }
    assert!(g.num("me.guns[1]") > 0.0, "a gun in slot 2: {}", g.probe("me.guns"));
    assert!(g.num("me.mag[1]") > 0.0, "loaded");
}

#[test]
fn the_vault_opens_in_the_third_minute() {
    let mut g = Game::new();
    g.exec("stage = \"playing\"\nround_time = 179.9");
    assert_eq!(g.probe("gate_open"), "false");
    g.run(12, &[], &[]);
    assert_eq!(g.probe("gate_open"), "true");
    assert_eq!(g.probe("hud.banners[hud.banners.length - 1][0]"), "THE SIGNAL VAULT IS OPEN");
}

#[test]
fn a_round_plays_out_to_one_left_standing() {
    let mut g = Game::new();
    // Ride until everyone has dropped, then step out of the fight.
    g.run(60 * 40, &[], &[]);
    assert_eq!(g.probe("anyone_aboard()"), "false", "everyone dropped");
    g.exec("me.alive = false");
    g.run(2, &[], &[]);
    assert_eq!(g.probe("stage"), "over");
    // The bots fight on as the Hush closes; well within a long round,
    // at most one is left.
    let start = std::time::Instant::now();
    let mut minutes = 0;
    while minutes < 12 && g.num("[b for b in entities.of(bots.Bot) if b.alive].length") > 1.0 {
        g.run(60 * 60, &[], &[]);
        minutes += 1;
    }
    let left = g.num("[b for b in entities.of(bots.Bot) if b.alive].length");
    let per_tick = start.elapsed().as_secs_f64() * 1000.0 / (f64::from(minutes.max(1)) * 3600.0);
    eprintln!("round over after ~{minutes} more minutes: {left} left; hush phase {}; {per_tick:.3} ms/tick", g.probe("hush.phase"));
    assert!(left <= 1.0, "one (or none) left after {minutes} minutes: {left}");
}

#[test]
fn play_again_starts_a_fresh_round() {
    let mut g = Game::new();
    g.run(30, &[], &[]);
    // The keeper falls mid-round: the round ends and the results menu
    // comes up (ignoring input for a moment, so a held click can't skip it).
    g.exec("stage = \"playing\"\nme.alive = false");
    g.run(2, &[], &[]);
    assert_eq!(g.probe("stage"), "over");
    assert_eq!(g.probe("menu"), "over");
    g.run(1, &[], &["r"]);
    assert_eq!(g.probe("stage"), "over", "too soon");
    g.run(60, &[], &[]);
    g.run(1, &[], &["r"]);
    assert_eq!(g.probe("stage"), "barge");
    assert_eq!(g.probe("me.phase"), "aboard");
    assert_eq!(g.probe("me.alive"), "true");
    assert_eq!(g.num("entities.count(bots.Bot)"), 31.0);
    assert_eq!(g.num("combat.feed.length"), 0.0);
}

/// A round in play, rendered for eyes-on review
/// (`target/lanternfall/round.png`): landed, the Hush closing, loot about.
#[test]
fn a_round_renders() {
    let mut renderer = match pollster::block_on(twec::kernel::render::Renderer::new_headless(960, 540)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let mut g = Game::new();
    g.input.set_view(960, 540);
    g.run(60 * 40, &[], &[]);
    // Fast-forward into the first shrink, and open the nearest chest.
    g.exec("hush.timer = 1.0\nlet c = loot.nearest_chest(me.pos, 60.0)\nif c != nil:\n    loot.open_chest(c)\n    me.pos = me.pos");
    g.run(60 * 6, &[], &[]);
    let mut assets = twec::play3d::NativeAssets::default();
    for _ in 0..200 {
        twec::host3d::render_frame(&mut renderer, &mut g.env, &mut assets).expect("render");
        if assets.pending() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    twec::host3d::render_frame(&mut renderer, &mut g.env, &mut assets).expect("render");
    let rgba = renderer.read_pixels().expect("pixels");
    std::fs::create_dir_all("target/lanternfall").unwrap();
    image::save_buffer("target/lanternfall/round.png", &rgba, 960, 540, image::ColorType::Rgba8).unwrap();
}

#[test]
fn held_guns_follow_their_fighters() {
    let mut g = Game::new();
    g.practice();
    g.run(3, &[], &[]);
    // The pistol is drawn at the keeper's hands, facing where it faces.
    assert_eq!(g.probe("my_gun.shown"), "true");
    assert_eq!(g.num("my_gun.gun"), 1.0);
    assert!(g.num("math.length(my_gun.pos - me.pos)") < 1.5);
    assert_eq!(g.num("my_gun.heading"), g.num("me.heading"));
    // The wrench in hand: the gun is put away.
    g.exec("me.slot = 0");
    g.run(2, &[], &[]);
    assert_eq!(g.probe("my_gun.shown"), "false");
    // Every rival carries one too, gone when the rival is.
    let mut r = Game::new();
    r.run(2, &[], &[]);
    assert_eq!(r.num("entities.count(loot.HeldGun)"), 32.0);
    r.no_rivals();
    r.run(2, &[], &[]);
    assert_eq!(r.num("entities.count(loot.HeldGun)"), 1.0, "only the keeper's is left");
}

#[test]
fn hits_mark_the_crosshair_and_point_at_the_shooter() {
    let mut g = Game::new();
    g.practice();
    let d = "entities.of(combat.Dummy)[2]";
    g.exec(&format!("combat.hurt({d}, 10.0, me, {d}.pos)"));
    assert_eq!(g.num("me.hit_marker"), 1.0);
    g.exec(&format!("combat.hurt(me, 10.0, {d}, me.pos)"));
    assert_eq!(g.num("me.hit_from_t"), 1.0);
    assert_eq!(g.num("me.hit_from.z"), g.num(&format!("{d}.pos.z")));
    // Both fade.
    g.run(90, &[], &[]);
    assert_eq!(g.num("me.hit_marker"), 0.0);
    assert!(g.num("me.hit_from_t") < 0.1);
    // Low health turns the screen's edge red.
    g.exec("me.hp = 10.0");
    g.run(2, &[], &[]);
    assert!(twec::stdlib::vignette_strength() >= 0.5);
    assert!(twec::stdlib::vignette_color()[0] > 0.4, "a red edge");
}

#[test]
fn the_last_moment_plays_in_slow_motion() {
    let mut g = Game::new();
    g.practice();
    // Everyone else gone and the round live: the keeper wins, slowly.
    g.exec("for d in entities.of(combat.Dummy):\n    despawn d\nstage = \"playing\"");
    g.run(2, &[], &[]);
    assert_eq!(g.probe("won"), "true");
    assert_eq!(g.num("time.scale"), 0.3);
    // 1.8 real seconds later the world is back to speed.
    g.run(130, &[], &[]);
    assert_eq!(g.num("time.scale"), 1.0);
    // And a fresh round never starts slowed.
    g.exec("time.set_scale(0.5)\nstart_round()");
    assert_eq!(g.num("time.scale"), 1.0);
}

/// Press `key` for one tick, then let go for one.
fn tap(g: &mut Game, key: &str) {
    g.run(1, &[], &[key]);
    g.run(1, &[], &[]);
}

#[test]
fn the_game_opens_on_the_title_and_plays_from_it() {
    let mut g = Game::title();
    g.run(20, &[], &[]);
    assert_eq!(g.probe("stage"), "title");
    assert_eq!(g.probe("menu"), "title");
    assert_eq!(g.num("entities.count(bots.Bot)"), 0.0, "an empty island behind the title");
    assert_eq!(g.probe("me.control"), "false");
    // The camera drifts round the island.
    let x0 = g.num("camera.eye.x");
    g.run(60, &[], &[]);
    assert!((g.num("camera.eye.x") - x0).abs() > 0.5);
    // Down, up, then Enter on Play.
    tap(&mut g, "down");
    assert_eq!(g.num("menus.focus"), 1.0);
    tap(&mut g, "up");
    assert_eq!(g.num("menus.focus"), 0.0);
    tap(&mut g, "enter");
    assert_eq!(g.probe("stage"), "barge");
    assert_eq!(g.probe("menu"), "");
    assert_eq!(g.num("entities.count(bots.Bot)"), 31.0);
}

#[test]
fn the_mouse_works_the_menus() {
    let mut renderer = match pollster::block_on(twec::kernel::render::Renderer::new_headless(640, 360)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let mut g = Game::title();
    g.input.set_view(640, 360);
    g.run(20, &[], &[]);
    // A frame lays the menu out; then hover Settings and click it.
    let mut assets = twec::play3d::NativeAssets::default();
    twec::host3d::render_frame(&mut renderer, &mut g.env, &mut assets).expect("render");
    let x = g.num("menus.rects[1][0] + menus.rects[1][2] / 2");
    let y = g.num("menus.rects[1][1] + menus.rects[1][3] / 2");
    let w = g.num("screen.size.x");
    g.input.pointer(x / w, y / 480.0);
    g.run(2, &[], &[]);
    assert_eq!(g.num("menus.focus"), 1.0, "hovering picks the row");
    g.input.button_down("left");
    g.run(1, &[], &[]);
    g.input.button_up("left");
    g.run(1, &[], &[]);
    assert_eq!(g.probe("menu"), "settings");
}

#[test]
fn escape_pauses_the_round_and_freezes_the_world() {
    let mut g = Game::new();
    g.run(20, &[], &[]);
    tap(&mut g, "escape");
    assert_eq!(g.probe("menu"), "pause");
    assert_eq!(g.probe("is_paused()"), "true");
    // Nothing moves while paused.
    let at = g.probe("barge.pos");
    g.run(60, &[], &[]);
    assert_eq!(g.probe("barge.pos"), at);
    // Esc again resumes, and the trigger ignores the mouse a moment.
    tap(&mut g, "escape");
    assert_eq!(g.probe("menu"), "");
    assert_eq!(g.probe("is_paused()"), "false");
    assert!(g.num("me.calm") > 0.0);
    g.run(5, &[], &[]);
    assert_ne!(g.probe("barge.pos"), at, "the barge sails on");
    // Pause, then leave the round from the menu (down twice, Enter).
    tap(&mut g, "escape");
    g.run(8, &[], &[]);
    tap(&mut g, "down");
    tap(&mut g, "down");
    tap(&mut g, "enter");
    assert_eq!(g.probe("stage"), "title");
    assert_eq!(g.probe("is_paused()"), "false");
    assert_eq!(g.num("entities.count(bots.Bot)"), 0.0);
}

#[test]
fn keys_rebind_from_settings_and_stick() {
    let mut g = Game::title();
    g.run(12, &[], &[]);
    tap(&mut g, "down");
    tap(&mut g, "enter");
    assert_eq!(g.probe("menu"), "settings");
    g.run(8, &[], &[]);
    // Row 5 is "Move up" (after three volumes, graphics, shake).
    for _ in 0..5 {
        tap(&mut g, "down");
    }
    assert_eq!(g.probe("menus.items[menus.focus][0]"), "key.up");
    tap(&mut g, "enter");
    assert_eq!(g.num("menus.listening"), 5.0);
    tap(&mut g, "i");
    assert_eq!(g.probe("controls.keys[controls.UP]"), "i");
    assert_eq!(g.num("menus.listening"), -1.0);
    // Turn the music down one step (row 1).
    for _ in 0..4 {
        tap(&mut g, "up");
    }
    tap(&mut g, "left");
    assert!((g.num("sounds.music_volume") - 0.5).abs() < 1e-9);
    // Esc closes settings and saves them.
    tap(&mut g, "escape");
    assert_eq!(g.probe("menu"), "title");
    let path = g.probe("SETTINGS_PATH");
    let saved = std::fs::read_to_string(&path).expect("settings saved");
    assert!(saved.contains("\"i\""), "{saved}");
    // In a round the new key walks the keeper north, and W doesn't.
    g.exec("act_on(\"play\")");
    g.practice();
    let z0 = g.num("me.pos.z");
    g.run(30, &["w"], &[]);
    assert!((g.num("me.pos.z") - z0).abs() < 0.05, "W is no longer bound");
    g.run(30, &["i"], &[]);
    assert!(g.num("me.pos.z") < z0 - 1.0, "I walks north");
}

#[test]
fn a_finished_round_is_remembered() {
    let mut g = Game::new();
    g.practice();
    g.exec("for d in entities.of(combat.Dummy):\n    despawn d\nme.kills = 3\nstage = \"playing\"");
    g.run(2, &[], &[]);
    assert_eq!(g.probe("won"), "true");
    assert_eq!(g.num("best_place"), 1.0);
    assert_eq!(g.probe("new_best"), "true");
    let path = g.probe("RECORD_PATH");
    assert!(std::path::Path::new(&path).exists(), "{path}");
    // Play again and lose: the best stays.
    g.run(60, &[], &[]);
    tap(&mut g, "enter");
    assert_eq!(g.probe("stage"), "barge");
    g.exec("stage = \"playing\"\nme.alive = false");
    g.run(2, &[], &[]);
    assert_eq!(g.probe("new_best"), "false");
    assert_eq!(g.num("best_place"), 1.0);
    // What's on disk is what a fresh boot reads.
    g.exec("save.clear()\nsave.try_read(RECORD_PATH)");
    assert_eq!(g.num("save.get_int(\"best_place\")"), 1.0);
    assert_eq!(g.num("save.get_int(\"rounds\")"), 2.0);
    assert_eq!(g.num("save.get_int(\"best_kills\")"), 3.0);
}

/// The menus, rendered for eyes-on review: `target/lanternfall/menu_*.png`.
#[test]
fn the_menus_render() {
    let mut renderer = match pollster::block_on(twec::kernel::render::Renderer::new_headless(960, 540)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let mut g = Game::title();
    g.input.set_view(960, 540);
    g.run(90, &[], &[]);
    let mut assets = twec::play3d::NativeAssets::default();
    let mut shot = |g: &mut Game, name: &str| {
        for _ in 0..200 {
            twec::host3d::render_frame(&mut renderer, &mut g.env, &mut assets).expect("render");
            if assets.pending() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        twec::host3d::render_frame(&mut renderer, &mut g.env, &mut assets).expect("render");
        let rgba = renderer.read_pixels().expect("pixels");
        std::fs::create_dir_all("target/lanternfall").unwrap();
        image::save_buffer(format!("target/lanternfall/menu_{name}.png"), &rgba, 960, 540, image::ColorType::Rgba8).unwrap();
    };
    shot(&mut g, "title");
    tap(&mut g, "down");
    tap(&mut g, "enter");
    g.run(8, &[], &[]);
    tap(&mut g, "down");
    tap(&mut g, "down");
    tap(&mut g, "down");
    shot(&mut g, "settings");
    tap(&mut g, "escape");
    g.run(8, &[], &[]);
    tap(&mut g, "up");
    tap(&mut g, "enter");
    assert_eq!(g.probe("stage"), "barge");
    g.run(400, &[], &[]);
    tap(&mut g, "escape");
    shot(&mut g, "pause");
    tap(&mut g, "escape");
    g.practice();
    g.exec("me.kills = 4\nme.damage = 412.0\nround_time = 263.0\nfor d in entities.of(combat.Dummy):\n    despawn d\nstage = \"playing\"");
    g.run(150, &[], &[]);
    shot(&mut g, "results");
}

/// G8: where a tick and a frame's CPU time go, mid-round with everyone
/// landed. `cargo test --release --test lanternfall -- --ignored perf --nocapture`.
#[test]
#[ignore]
fn perf_breakdown() {
    let mut g = Game::new();
    // Everyone leaps and lands; the round runs a while.
    g.run(60 * 50, &[], &["space"]);
    eprintln!("frozen in the first 50 s: {:.0} of 3000 ticks", 3000.0 - g.env.sim_time / DT);
    let n = 600;
    let sim0 = g.env.sim_time;
    let t = std::time::Instant::now();
    g.run(n, &[], &[]);
    let tick = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    // Ticks the game stood frozen (hit-stop) out of the n.
    let frozen = n as f64 - (g.env.sim_time - sim0) / DT;
    eprintln!("frozen by hit-stop: {frozen:.0} of {n} ticks");
    // The same ticks again under the profiler: inclusive time per
    // function / method, per tick (the profiler adds its own overhead).
    twec::profile::enable();
    g.run(n, &[], &[]);
    twec::profile::disable();
    let trace = std::path::Path::new("target/lanternfall/perf_trace.json");
    twec::profile::dump_to_path(trace).expect("trace");
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(trace).unwrap()).unwrap();
    let mut by: std::collections::HashMap<String, (f64, u64)> = Default::default();
    for e in doc["traceEvents"].as_array().unwrap() {
        let name = e["name"].as_str().unwrap_or("?").to_string();
        let d = e["dur"].as_f64().unwrap_or(0.0);
        let slot = by.entry(name).or_default();
        slot.0 += d;
        slot.1 += 1;
    }
    let mut rows: Vec<_> = by.into_iter().collect();
    rows.sort_by(|a, b| b.1 .0.partial_cmp(&a.1 .0).unwrap());
    for (name, (us, count)) in rows.iter().take(30) {
        eprintln!("  {:>8.3} ms/tick  {:>7} calls/tick  {name}", us / 1000.0 / n as f64, count / n as u64);
    }
    let frames = 300;
    let t = std::time::Instant::now();
    for _ in 0..frames {
        twec::eval::render_frame3d(&mut g.env).expect("frame");
    }
    let frame = t.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    let hud = g.env.top_on_render.take();
    let t = std::time::Instant::now();
    for _ in 0..frames {
        twec::eval::render_frame3d(&mut g.env).expect("frame");
    }
    let looks = t.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    g.env.top_on_render = hud;
    let stage = g.probe("stage");
    let alive = g.probe("alive_count()");
    let cache = g.env.look_cache.as_ref().unwrap();
    let shared: usize = cache.shared.iter().map(|s| s.1.len()).sum();
    eprintln!(
        "stage {} alive {} | tick {tick:.2} ms | frame cpu {frame:.2} ms (looks {looks:.2}, hud {:.2}) | entities {} tickable {} | looks own {} shared {} in {} classes | draws {}",
        stage,
        alive,
        frame - looks,
        g.env.active_entities.len(),
        g.env.tickable_entities.len(),
        cache.own.len(),
        shared,
        cache.shared.len(),
        g.env.render_queue3d.len()
    );
}

/// G8: a frame's renderer cost, mid-round. Run with `TWE_GPU_POWER=low`
/// (the integrated GPU) and `TWE_GPU_PROFILE=1` for per-pass GPU times:
/// `cargo test --release --test lanternfall -- --ignored perf_render --nocapture`.
#[test]
#[ignore]
fn perf_render() {
    let mut renderer = match pollster::block_on(twec::kernel::render::Renderer::new_headless(960, 720)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let mut g = Game::new();
    g.input.set_view(960, 720);
    g.exec("graphics.quality(\"high\")");
    g.run(60 * 50, &[], &["space"]);
    // An experiment to apply first (a Twe snippet), e.g. "postfx.ao(0.0)".
    if let Ok(src) = std::env::var("PERF_EXEC") {
        g.exec(&src.replace("\\n", "\n"));
    }
    let mut assets = twec::play3d::NativeAssets::default();
    for _ in 0..300 {
        twec::host3d::render_frame(&mut renderer, &mut g.env, &mut assets).expect("render");
        if assets.pending() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Warm, then time frames with a tick between each (as when playing).
    for _ in 0..30 {
        g.run(1, &[], &[]);
        twec::host3d::render_frame(&mut renderer, &mut g.env, &mut assets).expect("render");
    }
    let n = 120;
    let mut script = 0.0;
    let mut kernel = 0.0;
    let t = std::time::Instant::now();
    for _ in 0..n {
        g.run(1, &[], &[]);
        let times = twec::host3d::render_frame(&mut renderer, &mut g.env, &mut assets).expect("render");
        script += times.script_ms;
        kernel += times.kernel_ms;
    }
    let wall = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    eprintln!(
        "frame wall {wall:.2} ms (incl. tick) | script render {:.2} ms | kernel {:.2} ms | draws {}",
        script / n as f64,
        kernel / n as f64,
        g.env.render_queue3d.len(),
    );
}

/// From the title, as a player does it: Play, leap, land, punch, take a gun.
#[test]
fn from_the_title_the_keeper_can_punch_and_take_guns() {
    let mut g = Game::title();
    g.run(20, &[], &[]);
    tap(&mut g, "enter");
    assert_eq!(g.probe("stage"), "barge");
    g.run(90, &[], &[]);
    tap(&mut g, "space");
    let mut landed = false;
    for _ in 0..60 * 40 {
        g.run(1, &[], &[]);
        if g.probe("me.phase") == "ground" {
            landed = true;
            break;
        }
    }
    assert!(landed, "landed");
    // Just landed: the hint says how to fight and loot.
    let hint = g.probe("sound.now() - me.landed_at < 8.0 and me.guns[1] < 0");
    assert_eq!(hint, "true");
    g.run(30, &[], &[]);
    // A target a couple of metres off, a little to one side of where the
    // cursor points (up the screen, north): a click swings the wrench
    // and it connects.
    g.exec("let d = spawn combat.Dummy at me.pos + vec3(0.9, 0, -2.2)\nd.shield = 0.0");
    g.run(2, &[], &[]);
    let hp0 = g.num("entities.of(combat.Dummy)[0].hp");
    g.input.button_down("left");
    g.run(3, &[], &[]);
    g.input.button_up("left");
    assert_eq!(g.probe("mesh_anim.current(me).clip"), "melee", "the wrench swung");
    assert!(g.num("entities.of(combat.Dummy)[0].hp") < hp0, "and hit");
    // A gun at the keeper's feet with a free slot: walking onto it takes it.
    g.exec("loot.spill(me.pos, loot.WEAPON, 1, 1, 0)");
    g.run(60, &[], &[]);
    g.exec("for l in entities.of(loot.Loot):\n    l.pos = me.pos");
    g.run(2, &[], &[]);
    assert!(g.num("me.guns[1]") > 0.0, "took the gun without a key: {}", g.probe("me.guns"));
    // Both slots full: a gun on the ground needs E, to swap.
    g.exec("me.guns = [0, 1, 2]\nme.slot = 1\nloot.spill(me.pos, loot.WEAPON, 4, 2, 0)");
    g.run(60, &[], &[]);
    g.exec("for l in entities.of(loot.Loot):\n    l.pos = me.pos");
    g.run(2, &[], &[]);
    assert!(g.probe("me.prompt").starts_with("E  swap for"), "{}", g.probe("me.prompt"));
    tap(&mut g, "e");
    assert_eq!(g.num("me.guns[1]"), 4.0, "swapped: {}", g.probe("me.guns"));
}
