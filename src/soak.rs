//! web3d-M4: the soak harness — a scripted player for long unattended
//! runs of a 3D game (`examples/survive3d` is the one it was written
//! for).
//!
//! The same driver runs natively (`tests/soak.rs`) and in WebAssembly
//! (`twe-web`'s `soak` export, run under Node by `web/soak.mjs`), so an
//! interpreter or GC fault that only shows up after minutes of play, or
//! only on one target, surfaces the same way on both. Input goes
//! through `host3d::InputState` + `host3d::sim_tick` and each tick runs
//! the 3D render script, exactly as the shells do; only the GPU is
//! missing.
//!
//! The bot knows the scene states `survive3d` uses: it moves in a
//! weaving circle while `playing`, picks the first card at `level_up`,
//! resumes from `paused`, and presses R at `game_over` so the soak
//! spans many runs. Every 90 s of play it pauses for a second.
//! `variant` changes its path (segment length, heading, sidesteps), so
//! consecutive soaks of a deterministic game are different sessions.
//!
//! Lanternfall G8: a second pilot, [`Pilot::Lanternfall`], plays the
//! battle royale — Play on the title, leap from the barge, roam, aim
//! around and fire, open and take with E, Starcall, pause every 90 s,
//! Play again when a round ends — reading its `stage` / `menu` globals.
//! [`run_env`] takes an env the caller loaded, so a multi-file game
//! (modules from disk natively, from its bundle in wasm) soaks too.

use crate::host3d::{sim_tick, InputState};
use crate::value::Env;

/// What a soak did, for the log and for assertions.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SoakReport {
    pub ticks: u64,
    /// Runs that ended in `game_over` (the bot restarts each one).
    pub deaths: u32,
    pub level_ups: u32,
    pub pauses: u32,
    /// Most live entities seen on any tick.
    pub peak_entities: usize,
    /// GC heap bytes alive after each restart, measured after a full
    /// collection: flat means no leak across runs.
    pub heap_at_restart: Vec<usize>,
    pub peak_heap: usize,
    /// Lanternfall: rounds finished, and how many the pilot won.
    pub rounds: u32,
    pub wins: u32,
}

/// Which scripted player drives the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pilot {
    /// `examples/survive3d`'s scene states (the M4 soak).
    Survive3d,
    /// `examples/lanternfall`'s stages and menus (G8).
    Lanternfall,
}

impl Pilot {
    pub fn parse(name: &str) -> Option<Pilot> {
        match name {
            "survive3d" => Some(Pilot::Survive3d),
            "lanternfall" => Some(Pilot::Lanternfall),
            _ => None,
        }
    }
}

impl SoakReport {
    /// One line of JSON (the wasm export returns this to Node).
    pub fn to_json(&self) -> String {
        let heaps: Vec<String> = self.heap_at_restart.iter().map(|b| b.to_string()).collect();
        format!(
            r#"{{"ticks":{},"deaths":{},"level_ups":{},"pauses":{},"rounds":{},"wins":{},"peak_entities":{},"peak_heap":{},"heap_at_restart":[{}]}}"#,
            self.ticks,
            self.deaths,
            self.level_ups,
            self.pauses,
            self.rounds,
            self.wins,
            self.peak_entities,
            self.peak_heap,
            heaps.join(",")
        )
    }
}

fn state(env: &Env) -> String {
    env.active_scene
        .as_ref()
        .and_then(|s| s.borrow().current_state.clone())
        .unwrap_or_default()
}

/// Load `source`, then play `ticks` fixed ticks with the bot. Asset
/// reads go wherever the caller pointed them (an asset root natively, a
/// mounted bundle in wasm). Any runtime error ends the soak with the
/// tick it happened on.
pub fn run(source: &str, ticks: u64, variant: u64) -> Result<SoakReport, String> {
    let tokens = crate::lexer::lex(source).map_err(|e| e.to_string())?;
    let program = crate::parser::parse(&tokens).map_err(|e| e.to_string())?;
    let mut env = Env::new();
    crate::stdlib::install(&mut env);
    crate::eval::run_top_level(&mut env, &program).map_err(|e| format!("top level: {e}"))?;
    run_env(env, ticks, variant, Pilot::Survive3d)
}

/// Play `ticks` fixed ticks of an already-loaded game with `pilot`.
pub fn run_env(env: Env, ticks: u64, variant: u64, pilot: Pilot) -> Result<SoakReport, String> {
    match pilot {
        Pilot::Survive3d => run_survive3d(env, ticks, variant),
        Pilot::Lanternfall => run_lanternfall(env, ticks, variant),
    }
}

fn run_survive3d(mut env: Env, ticks: u64, variant: u64) -> Result<SoakReport, String> {
    let mut input = InputState::default();
    let mut report = SoakReport::default();
    let dirs = ["w", "d", "s", "a"];
    let mut held: Option<&'static str> = None;
    let mut played: u64 = 0;
    for tick in 0..ticks {
        let st = state(&env);
        let mut tap: Option<&'static str> = None;
        let mut want: Option<&'static str> = None;
        match st.as_str() {
            "playing" => {
                played += 1;
                if played.is_multiple_of(90 * 60) {
                    tap = Some("escape");
                } else {
                    // A weaving circle: turn every 1-2 s, sidestepping
                    // every few segments.
                    let seg = (played / (60 + 13 * (variant % 5))) as usize + variant as usize;
                    let every = 2 + (variant % 3) as usize;
                    want = Some(if seg % every == every - 1 {
                        dirs[(seg + 1) % 4]
                    } else {
                        dirs[seg % 4]
                    });
                }
            }
            "level_up" => {
                tap = Some("1");
                report.level_ups += 1;
            }
            "paused" => {
                if tick.is_multiple_of(60) {
                    tap = Some("escape");
                    report.pauses += 1;
                }
            }
            "game_over" => {
                if tick.is_multiple_of(30) {
                    tap = Some("r");
                }
            }
            _ => {}
        }
        if held != want {
            if let Some(k) = held {
                input.key_up(k);
            }
            if let Some(k) = want {
                input.key_down(k);
            }
            held = want;
        }
        if let Some(k) = tap {
            input.key_down(k);
        }
        sim_tick(&mut env, &mut input, crate::eval::PHYSICS_DT)
            .map_err(|e| format!("tick {tick} ({st}): {e}"))?;
        crate::eval::render_frame3d(&mut env).map_err(|e| format!("render at tick {tick}: {e}"))?;
        env.out.clear();
        if let Some(k) = tap {
            input.key_up(k);
        }

        let now = state(&env);
        if st == "playing" && now == "game_over" {
            report.deaths += 1;
        }
        if st == "game_over" && now == "playing" {
            report.heap_at_restart.push(full_collect(&env));
        }
        report.peak_entities = report.peak_entities.max(env.active_entities.len());
        report.peak_heap = report.peak_heap.max(crate::heap::gc_bytes_alive());
        report.ticks = tick + 1;
    }
    Ok(report)
}

/// A global string (`""` if it isn't one).
fn global_str(env: &Env, name: &str) -> String {
    match env.get(name) {
        Some(v) if v.is_str() => v.as_string().to_string(),
        _ => String::new(),
    }
}

fn global_bool(env: &Env, name: &str) -> bool {
    matches!(env.get(name), Some(v) if v.is_bool() && v.as_bool())
}

/// The keeper's `phase` in Lanternfall ("aboard", "glide", "ground").
fn keeper_phase(env: &Env) -> String {
    env.get("me")
        .filter(|v| v.is_instance())
        .and_then(|v| v.as_instance().borrow().get_field("phase"))
        .filter(|v| v.is_str())
        .map(|v| v.as_string().to_string())
        .unwrap_or_default()
}

/// Lanternfall's pilot. Menus: Enter on the title and the results, Esc
/// out of a pause. Aboard: Space after a variant-dependent wait. On the
/// island: a weaving walk, the cursor circling the keeper, the trigger
/// held in bursts, E often (chests, guns), R and Q now and then, and a
/// pause every 90 s.
fn run_lanternfall(mut env: Env, ticks: u64, variant: u64) -> Result<SoakReport, String> {
    let mut input = InputState::default();
    input.set_view(1280, 720);
    let mut report = SoakReport::default();
    let dirs = ["w", "d", "s", "a"];
    let mut held: Option<&'static str> = None;
    let mut firing = false;
    let mut played: u64 = 0;
    let mut aboard: u64 = 0;
    for tick in 0..ticks {
        let stage = global_str(&env, "stage");
        let menu = global_str(&env, "menu");
        let mut taps: Vec<&'static str> = Vec::new();
        let mut want: Option<&'static str> = None;
        let mut fire = false;
        if menu == "pause" {
            if tick.is_multiple_of(60) {
                taps.push("escape");
            }
        } else if stage == "title" || stage == "over" {
            if tick.is_multiple_of(30) {
                taps.push("enter");
            }
        } else if stage == "barge" || stage == "playing" {
            played += 1;
            if keeper_phase(&env) == "aboard" {
                aboard += 1;
                if aboard == 60 + 41 * (variant % 7) {
                    taps.push("space");
                }
            } else {
                aboard = 0;
                let seg = (played / (70 + 11 * (variant % 5))) as usize + variant as usize;
                want = Some(dirs[seg % 4]);
                let a = played as f64 * 0.02 + variant as f64;
                input.pointer(0.5 + 0.25 * a.cos(), 0.5 + 0.3 * a.sin());
                fire = played % 180 < 110;
                if played.is_multiple_of(15) {
                    taps.push("e");
                }
                if played.is_multiple_of(301) {
                    taps.push("r");
                }
                if played.is_multiple_of(911) {
                    taps.push("q");
                }
                if played.is_multiple_of(90 * 60) {
                    taps.push("escape");
                    report.pauses += 1;
                }
            }
        }
        if held != want {
            if let Some(k) = held {
                input.key_up(k);
            }
            if let Some(k) = want {
                input.key_down(k);
            }
            held = want;
        }
        if fire != firing {
            if fire {
                input.button_down("left");
            } else {
                input.button_up("left");
            }
            firing = fire;
        }
        for k in &taps {
            input.key_down(k);
        }
        sim_tick(&mut env, &mut input, crate::eval::PHYSICS_DT)
            .map_err(|e| format!("tick {tick} ({stage}): {e}"))?;
        crate::eval::render_frame3d(&mut env).map_err(|e| format!("render at tick {tick}: {e}"))?;
        env.out.clear();
        for k in &taps {
            input.key_up(k);
        }

        let now = global_str(&env, "stage");
        if stage == "playing" && now == "over" {
            report.rounds += 1;
            if global_bool(&env, "won") {
                report.wins += 1;
            } else {
                report.deaths += 1;
            }
        }
        if stage == "over" && now == "barge" {
            report.heap_at_restart.push(full_collect(&env));
        }
        report.peak_entities = report.peak_entities.max(env.active_entities.len());
        report.peak_heap = report.peak_heap.max(crate::heap::gc_bytes_alive());
        report.ticks = tick + 1;
    }
    Ok(report)
}

/// Collect everything unreachable now (finishing any incremental cycle
/// in flight, then one whole cycle) and return the bytes still alive.
/// Only sound between ticks, where the env's roots are all there is.
fn full_collect(env: &Env) -> usize {
    let budget = crate::heap::gc_budget_ns();
    crate::heap::gc_set_budget_ns(u64::MAX);
    crate::heap::gc_collect_with(|| env.scan_roots());
    crate::heap::gc_collect_with(|| env.scan_roots());
    crate::heap::gc_set_budget_ns(budget);
    crate::heap::gc_bytes_alive()
}
