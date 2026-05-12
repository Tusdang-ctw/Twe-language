//! `fx.*` — procedural VFX library (v1.0.1 Session 1).
//!
//! Survivors-class game feel in one function call per effect. All
//! effects are procedural: no PNGs, no shaders, no cloud assets.
//! State lives in a thread_local list of `ActiveFx` that decays
//! each render frame and renders directly via macroquad primitives.
//!
//! Determinism contract: visual fx (damage numbers, bursts, etc.)
//! decay on wall-clock `frame_dt` — they're rendering-only, never
//! observed by the script, so replay can diverge slightly in their
//! per-frame appearance without affecting gameplay. `hit_stop` is
//! the one gameplay-visible primitive; it counts in physics ticks
//! (`PHYSICS_DT` slices), not wall-clock seconds, so replay sees
//! the same number of skipped substeps regardless of host FPS.
//!
//! Screen-shake delegates to the existing `CAMERA_SHAKE` thread_local
//! in `stdlib.rs` — `fx.screen_shake(...)` is the new canonical surface;
//! the older `camera.shake(...)` remains for backwards-compat.

use std::cell::RefCell;

use macroquad::color::Color;

/// One effect instance. Visual-only — never read by scripts.
#[derive(Clone)]
pub struct ActiveFx {
    pub kind: FxKind,
    /// World-space position (the same coords sprites/rects use).
    pub pos: (f32, f32),
    /// Seconds since spawn. Drives the animation `t = age / lifetime`.
    pub age: f32,
    /// Total lifetime in seconds. Effect is dropped when `age >= lifetime`.
    pub lifetime: f32,
}

#[derive(Clone)]
pub enum FxKind {
    /// Tint flash over a sprite/entity rect (damage feedback).
    HitFlash {
        color: Color,
        width: f32,
        height: f32,
    },
    /// Floating damage number that rises and fades.
    DamageNumber { value: f64, color: Color },
    /// Larger, longer-lived crit text — yellow + exclamation.
    CritText { value: f64 },
    /// Radial particle burst — death explosion / enemy poof.
    Burst {
        color: Color,
        /// Each tuple is `(offset_x, offset_y, vx, vy)`. `vx`/`vy` are
        /// pixels/sec; an implicit downward "gravity" of 80 px/s² adds
        /// arc to bursts.
        particles: Vec<(f32, f32, f32, f32)>,
    },
    /// Expanding outlined circle — pickup confirmation.
    Pop { color: Color },
    /// Expanding ring — level-up / spell-cast tell.
    Ring {
        color: Color,
        radius_from: f32,
        radius_to: f32,
    },
    /// Cone splatter — blood / shrapnel in a forward direction.
    Splat { dir: (f32, f32), color: Color },
    /// Forward-cone yellow flash — gunfire / spell-tip.
    MuzzleFlash { dir: (f32, f32) },
    /// White expanding ring on the ground — boss slam / AoE.
    Shockwave { radius: f32 },
    /// Single trail dot — call once per frame to leave a streak.
    Trail { color: Color },
}

pub struct FxState {
    pub active: Vec<ActiveFx>,
    /// Physics-tick countdown for `fx.hit_stop`. Each substep in the
    /// play loop's accumulator drain decrements this; if non-zero,
    /// the substep is skipped (gameplay frozen, render continues).
    pub hit_stop_ticks: u32,
}

thread_local! {
    static FX: RefCell<FxState> = const { RefCell::new(FxState {
        active: Vec::new(),
        hit_stop_ticks: 0,
    }) };
}

/// Push a new effect onto the active list.
pub fn spawn(fx: ActiveFx) {
    FX.with(|s| s.borrow_mut().active.push(fx));
}

/// Schedule a hit-stop. `duration_seconds` is converted to
/// `ceil(duration / physics_dt)` ticks; stacking takes the max
/// (a longer hit-stop extends, a shorter one doesn't shorten).
pub fn schedule_hit_stop(duration_seconds: f64, physics_dt: f64) {
    let ticks = (duration_seconds / physics_dt).ceil().max(0.0) as u32;
    FX.with(|s| {
        let mut st = s.borrow_mut();
        st.hit_stop_ticks = st.hit_stop_ticks.max(ticks);
    });
}

/// Returns `true` iff a hit-stop tick is queued; decrements the
/// counter as a side-effect. Call from inside the physics substep
/// loop *before* `tick_frame`; skip the substep when this returns true.
pub fn consume_hit_stop_tick() -> bool {
    FX.with(|s| {
        let mut st = s.borrow_mut();
        if st.hit_stop_ticks > 0 {
            st.hit_stop_ticks -= 1;
            true
        } else {
            false
        }
    })
}

/// Snapshot for tests / debug HUDs.
pub fn active_count() -> usize {
    FX.with(|s| s.borrow().active.len())
}

/// Snapshot for tests.
pub fn hit_stop_ticks_remaining() -> u32 {
    FX.with(|s| s.borrow().hit_stop_ticks)
}

/// Wipe all state. Called by `clear_asset_caches` on hot-reload so
/// the new script doesn't inherit stale effects mid-stream.
pub fn clear() {
    FX.with(|s| {
        let mut st = s.borrow_mut();
        st.active.clear();
        st.hit_stop_ticks = 0;
    });
}

/// Decay every active effect by wall-clock `dt`. Call once per render
/// frame (after `camera_tick`, before the world render). Drops
/// effects whose age has reached or exceeded their lifetime.
pub fn fx_tick(dt: f64) {
    FX.with(|s| {
        let mut st = s.borrow_mut();
        let dt = dt as f32;
        for fx in &mut st.active {
            fx.age += dt;
        }
        st.active.retain(|fx| fx.age < fx.lifetime);
    });
}

/// Render every active effect using macroquad primitives. Called from
/// the play loop after the script's `on render():` has drawn its
/// world, so fx layers on top.
pub fn fx_draw_overlay() {
    FX.with(|s| {
        let st = s.borrow();
        for fx in &st.active {
            draw_one(fx);
        }
    });
}

fn draw_one(fx: &ActiveFx) {
    use macroquad::shapes::{draw_circle, draw_circle_lines, draw_rectangle};
    use macroquad::text::draw_text;

    let t = (fx.age / fx.lifetime).clamp(0.0, 1.0);
    let inv_t = 1.0 - t;

    match &fx.kind {
        FxKind::HitFlash {
            color,
            width,
            height,
        } => {
            let c = with_alpha(*color, inv_t);
            draw_rectangle(
                fx.pos.0 - width * 0.5,
                fx.pos.1 - height * 0.5,
                *width,
                *height,
                c,
            );
        }
        FxKind::DamageNumber { value, color } => {
            let c = with_alpha(*color, inv_t);
            let lift = -40.0 * t;
            let s = format!("{}", *value as i64);
            draw_text(&s, fx.pos.0, fx.pos.1 + lift, 22.0, c);
        }
        FxKind::CritText { value } => {
            let c = with_alpha(macroquad::color::YELLOW, inv_t);
            let lift = -60.0 * t;
            let s = format!("{}!", *value as i64);
            draw_text(&s, fx.pos.0, fx.pos.1 + lift, 32.0, c);
        }
        FxKind::Burst { color, particles } => {
            let c = with_alpha(*color, inv_t);
            let g = 80.0; // px/s² downward
            for (ox, oy, vx, vy) in particles {
                let x = fx.pos.0 + ox + vx * fx.age;
                let y = fx.pos.1 + oy + vy * fx.age + 0.5 * g * fx.age * fx.age;
                draw_circle(x, y, 2.5 * inv_t.max(0.0), c);
            }
        }
        FxKind::Pop { color } => {
            let c = with_alpha(*color, inv_t);
            let r = 6.0 + 18.0 * t;
            draw_circle_lines(fx.pos.0, fx.pos.1, r, 2.0, c);
        }
        FxKind::Ring {
            color,
            radius_from,
            radius_to,
        } => {
            let c = with_alpha(*color, inv_t);
            let r = radius_from + (radius_to - radius_from) * t;
            draw_circle_lines(fx.pos.0, fx.pos.1, r, 3.0, c);
        }
        FxKind::Splat { dir, color } => {
            let c = with_alpha(*color, inv_t);
            let (nx, ny) = normalize_or_zero(*dir);
            for i in 0..6 {
                let theta = ((i as f32) / 6.0 - 0.5) * 0.9;
                let (sn, cs) = theta.sin_cos();
                let dx = nx * cs - ny * sn;
                let dy = nx * sn + ny * cs;
                let r = 60.0 * fx.age;
                draw_circle(fx.pos.0 + dx * r, fx.pos.1 + dy * r, 3.0 * inv_t, c);
            }
        }
        FxKind::MuzzleFlash { dir } => {
            let (nx, ny) = normalize_or_zero(*dir);
            let c = with_alpha(Color::new(1.0, 0.9, 0.5, 1.0), inv_t);
            let r = 14.0 * inv_t;
            draw_circle(fx.pos.0 + nx * 12.0, fx.pos.1 + ny * 12.0, r, c);
        }
        FxKind::Shockwave { radius } => {
            let c = with_alpha(Color::new(1.0, 1.0, 1.0, 0.6), inv_t);
            let r = radius * t;
            let thickness = 4.0 * inv_t;
            draw_circle_lines(fx.pos.0, fx.pos.1, r, thickness.max(0.5), c);
        }
        FxKind::Trail { color } => {
            let c = with_alpha(*color, inv_t * 0.6);
            draw_circle(fx.pos.0, fx.pos.1, 4.0 * inv_t, c);
        }
    }
}

fn with_alpha(c: Color, a: f32) -> Color {
    Color::new(c.r, c.g, c.b, c.a * a.clamp(0.0, 1.0))
}

fn normalize_or_zero(v: (f32, f32)) -> (f32, f32) {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt();
    if len < 1e-6 {
        (0.0, 0.0)
    } else {
        (v.0 / len, v.1 / len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        clear();
    }

    #[test]
    fn tick_drops_expired() {
        reset();
        spawn(ActiveFx {
            kind: FxKind::Pop {
                color: Color::new(1.0, 1.0, 0.0, 1.0),
            },
            pos: (10.0, 10.0),
            age: 0.0,
            lifetime: 0.1,
        });
        assert_eq!(active_count(), 1);
        fx_tick(0.05);
        assert_eq!(active_count(), 1);
        fx_tick(0.06);
        assert_eq!(active_count(), 0);
    }

    #[test]
    fn hit_stop_counts_in_ticks() {
        reset();
        // 60 Hz physics dt = 1/60s. 0.05s = 3 ticks (ceil).
        schedule_hit_stop(0.05, 1.0 / 60.0);
        assert_eq!(hit_stop_ticks_remaining(), 3);
        assert!(consume_hit_stop_tick());
        assert!(consume_hit_stop_tick());
        assert!(consume_hit_stop_tick());
        assert!(!consume_hit_stop_tick());
    }

    #[test]
    fn hit_stop_stacks_to_max() {
        reset();
        schedule_hit_stop(0.05, 1.0 / 60.0); // 3 ticks
        schedule_hit_stop(0.10, 1.0 / 60.0); // 6 ticks — wins
        schedule_hit_stop(0.02, 1.0 / 60.0); // 2 ticks — ignored
        assert_eq!(hit_stop_ticks_remaining(), 6);
    }

    #[test]
    fn clear_wipes_state() {
        reset();
        spawn(ActiveFx {
            kind: FxKind::Pop {
                color: Color::new(1.0, 0.0, 0.0, 1.0),
            },
            pos: (0.0, 0.0),
            age: 0.0,
            lifetime: 1.0,
        });
        schedule_hit_stop(0.05, 1.0 / 60.0);
        clear();
        assert_eq!(active_count(), 0);
        assert_eq!(hit_stop_ticks_remaining(), 0);
    }
}
