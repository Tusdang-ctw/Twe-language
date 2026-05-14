//! `light2d.*` — dynamic 2D lighting (v1.0.1 Session 3).
//!
//! Cheap multi-light overlay for night-time / dungeon levels.
//! Scripts call `light2d.add(at, color, radius, flicker)` per frame
//! inside `on render():` for each torch / player-glow / muzzle light;
//! the play loop draws an ambient-darken pass + per-light glow circles
//! on top of the rendered world. Lights are cleared at end-of-frame —
//! same call-and-go shape as draw calls and the fx.* library.
//!
//! Determinism: the render-side state is visual-only (`light2d.add`
//! has NIL return). Scripts cannot observe the light buffer, so
//! replay parity is preserved by construction. Flicker is driven by
//! wall-clock `time` since the lights are visual; if a future
//! gameplay primitive ever *reads* light state it must move to
//! tick-aligned phase.
//!
//! Honest deferral: this MVP uses macroquad's default alpha-blend
//! pipeline — light overlays are colored alpha-blended circles, not
//! true additive light. The plan's "Cheap additive multi-light pass"
//! wording is approximated; a custom WGSL fragment-shader pass for
//! true additive blending is captured as a v1.0.2 follow-on. The
//! `light2d.cast_shadows(occluders)` builtin registers occluder
//! AABBs but the per-pixel shadow occlusion is deferred to the
//! WGSL follow-on (where it's a single fragment-shader loop).

use std::cell::RefCell;

use macroquad::color::Color;

/// Maximum lights per frame. Matches the 16-budget shape Phase 20
/// used for 3D point lights; over-budget calls are silently dropped
/// (with the count exposed via `dropped_count` for the HUD if needed).
pub const MAX_LIGHTS: usize = 16;

#[derive(Clone, Copy)]
pub struct Light2D {
    pub pos: (f32, f32),
    pub color: Color,
    pub radius: f32,
    /// 0.0 = no flicker; values up to ~0.3 give a torch-flame feel.
    /// Larger values produce more chaotic intensity wobble.
    pub flicker: f32,
}

#[derive(Clone, Copy)]
pub struct AabbOccluder {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub struct Light2DState {
    pub lights: Vec<Light2D>,
    pub dropped: u32,
    pub ambient: Color,
    pub shadow_aabbs: Vec<AabbOccluder>,
    /// Wall-clock seconds since the runtime started. Drives flicker
    /// phase. Advanced by `tick(dt)` once per render frame.
    pub time: f32,
}

thread_local! {
    static LIGHT_STATE: RefCell<Light2DState> = const { RefCell::new(Light2DState {
        lights: Vec::new(),
        dropped: 0,
        ambient: Color::new(0.0, 0.0, 0.0, 0.0),
        shadow_aabbs: Vec::new(),
        time: 0.0,
    }) };
}

/// Push a per-frame light. Silently dropped beyond `MAX_LIGHTS`.
pub fn add(light: Light2D) {
    LIGHT_STATE.with(|s| {
        let mut st = s.borrow_mut();
        if st.lights.len() >= MAX_LIGHTS {
            st.dropped = st.dropped.saturating_add(1);
            return;
        }
        st.lights.push(light);
    });
}

/// Set the ambient darkness overlay. `alpha = 0.0` disables the
/// overlay entirely. The RGB carries the tint of the unlit areas —
/// a deep blue for moonlight, near-black for caves, etc.
pub fn set_ambient(c: Color) {
    LIGHT_STATE.with(|s| s.borrow_mut().ambient = c);
}

pub fn ambient() -> Color {
    LIGHT_STATE.with(|s| s.borrow().ambient)
}

/// Register shadow occluder AABBs. The MVP only stashes them — see
/// the honest deferral note at the top of the module.
pub fn cast_shadows(aabbs: Vec<AabbOccluder>) {
    LIGHT_STATE.with(|s| s.borrow_mut().shadow_aabbs = aabbs);
}

pub fn shadow_aabb_count() -> usize {
    LIGHT_STATE.with(|s| s.borrow().shadow_aabbs.len())
}

/// Drop all per-frame lights but keep ambient / shadow state. Called
/// by the play loop at the end of each rendered frame.
pub fn clear_frame_lights() {
    LIGHT_STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.lights.clear();
        st.dropped = 0;
    });
}

/// Wipe every piece of state. Called by `clear_asset_caches` so a
/// hot-reload of the script doesn't inherit lights / ambient from
/// the previous run.
pub fn clear() {
    LIGHT_STATE.with(|s| {
        *s.borrow_mut() = Light2DState {
            lights: Vec::new(),
            dropped: 0,
            ambient: Color::new(0.0, 0.0, 0.0, 0.0),
            shadow_aabbs: Vec::new(),
            time: 0.0,
        };
    });
}

/// Advance the flicker phase. Wall-clock seconds.
pub fn tick(dt: f64) {
    LIGHT_STATE.with(|s| {
        s.borrow_mut().time += dt as f32;
    });
}

/// Snapshot for tests / debug HUDs.
pub fn active_count() -> usize {
    LIGHT_STATE.with(|s| s.borrow().lights.len())
}

/// Snapshot for tests / debug HUDs — the count of lights silently
/// dropped this frame because of the MAX_LIGHTS budget.
pub fn dropped_count() -> u32 {
    LIGHT_STATE.with(|s| s.borrow().dropped)
}

/// Pseudo-random in [-1, 1] for flicker. Deterministic on `(time, salt)`
/// so different lights jitter on different phases. Pure function —
/// no RNG state, no thread_local dice rolls.
fn pseudo_noise(time: f32, salt: f32) -> f32 {
    // Hash-y product chain. The point isn't perfect noise — just a
    // wobble that doesn't sync across lights.
    let phase = time * 11.13 + salt * 7.31;
    (phase.sin() * 0.6 + (phase * 2.7).sin() * 0.4).clamp(-1.0, 1.0)
}

/// Render the ambient overlay + each light's glow. Called by the
/// play loop AFTER the world has rendered, BEFORE the fx overlay.
/// `(view_x, view_y, view_w, view_h)` is the world-space rectangle
/// the ambient darkness should cover — typically the on-screen
/// portion of the world under the current camera transform.
pub fn draw_overlay(view_x: f32, view_y: f32, view_w: f32, view_h: f32) {
    use macroquad::shapes::{draw_circle, draw_rectangle};

    LIGHT_STATE.with(|s| {
        let st = s.borrow();
        if st.ambient.a > 0.0 {
            // Darken the scene with the ambient color. Alpha controls
            // how dim the unlit areas look.
            draw_rectangle(view_x, view_y, view_w, view_h, st.ambient);
        }
        for (i, light) in st.lights.iter().enumerate() {
            let mut intensity = 1.0_f32;
            if light.flicker > 0.0 {
                let salt = (i as f32) * 1.618;
                intensity = (1.0 + pseudo_noise(st.time, salt) * light.flicker).clamp(0.1, 1.6);
            }
            // 6 concentric rings give a soft falloff from the
            // light's color at center to transparent at radius.
            // The constants below are chosen to match macroquad's
            // default alpha-blend so the result looks like a glow
            // rather than a hard disc.
            let rings = 6;
            for r_step in 0..rings {
                let t_norm = (r_step as f32) / (rings as f32);
                let radius = light.radius * intensity * (1.0 - t_norm * 0.7);
                let alpha = light.color.a * (1.0 - t_norm) * 0.35;
                let c = Color::new(light.color.r, light.color.g, light.color.b, alpha);
                draw_circle(light.pos.0, light.pos.1, radius, c);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        clear();
    }

    #[test]
    fn add_pushes_lights_to_cap() {
        reset();
        for i in 0..(MAX_LIGHTS + 4) {
            add(Light2D {
                pos: (i as f32, 0.0),
                color: Color::new(1.0, 1.0, 1.0, 1.0),
                radius: 40.0,
                flicker: 0.0,
            });
        }
        assert_eq!(active_count(), MAX_LIGHTS);
        assert_eq!(dropped_count(), 4);
    }

    #[test]
    fn clear_frame_lights_keeps_ambient() {
        reset();
        set_ambient(Color::new(0.0, 0.0, 0.1, 0.7));
        add(Light2D {
            pos: (10.0, 10.0),
            color: Color::new(1.0, 0.6, 0.3, 1.0),
            radius: 80.0,
            flicker: 0.1,
        });
        assert_eq!(active_count(), 1);
        clear_frame_lights();
        assert_eq!(active_count(), 0);
        let a = ambient();
        assert!((a.a - 0.7).abs() < 1e-6);
    }

    #[test]
    fn clear_resets_everything() {
        reset();
        set_ambient(Color::new(0.1, 0.1, 0.2, 0.8));
        add(Light2D {
            pos: (5.0, 5.0),
            color: Color::new(1.0, 1.0, 1.0, 1.0),
            radius: 60.0,
            flicker: 0.0,
        });
        cast_shadows(vec![AabbOccluder {
            x: 0.0,
            y: 0.0,
            w: 32.0,
            h: 32.0,
        }]);
        clear();
        assert_eq!(active_count(), 0);
        assert_eq!(shadow_aabb_count(), 0);
        assert!(ambient().a < 1e-6);
    }

    #[test]
    fn flicker_is_deterministic_on_time_and_salt() {
        let a = pseudo_noise(0.5, 1.618);
        let b = pseudo_noise(0.5, 1.618);
        assert!((a - b).abs() < 1e-12);
        let c = pseudo_noise(0.5, 0.0);
        assert!((a - c).abs() > 1e-9);
    }

    #[test]
    fn flicker_bounded() {
        for i in 0..1000 {
            let t = i as f32 * 0.017;
            let v = pseudo_noise(t, 1.0);
            assert!((-1.0..=1.0).contains(&v));
        }
    }
}
