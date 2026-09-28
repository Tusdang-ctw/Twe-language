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
}

impl SoakReport {
    /// One line of JSON (the wasm export returns this to Node).
    pub fn to_json(&self) -> String {
        let heaps: Vec<String> = self.heap_at_restart.iter().map(|b| b.to_string()).collect();
        format!(
            r#"{{"ticks":{},"deaths":{},"level_ups":{},"pauses":{},"peak_entities":{},"peak_heap":{},"heap_at_restart":[{}]}}"#,
            self.ticks,
            self.deaths,
            self.level_ups,
            self.pauses,
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
