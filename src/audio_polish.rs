//! Audio polish — pooling + ducking + music layers (v1.0.1 Session 4).
//!
//! State + scheduling live here; the actual macroquad `play_sound` /
//! `set_sound_volume` calls happen inside `stdlib::play_sound_path`
//! after this module's pool / duck rules have applied. Per-frame
//! `tick(dt)` is invoked from the four `play.rs::run_loop*` variants.
//!
//! Honest deferral: macroquad's audio backend is "fire-and-forget" —
//! once a sound is playing we have no per-voice handle, only
//! `set_sound_volume(sound, vol)` that affects every active voice
//! of that sound. So:
//!
//!   - Pool max-voices is enforced as a *time-window throttle* on
//!     calls to `play`. A pool of `max_voices: 8` with a 1.0s default
//!     window will skip a 9th play in any rolling 1s. The user
//!     experience matches "200 overlapping plays don't all hit at
//!     once" without true voice-stealing.
//!   - Duck factor is applied to the volume passed to `play_sound`
//!     at call time; mid-play volume changes are not retro-applied
//!     to a sound that already started.
//!   - Crossfade ramps via `set_sound_volume`, which works because
//!     looped music tracks are single-voice in practice.
//!
//! True voice-handle audio (per-sound voice IDs + per-voice volume
//! ramps) is captured as a v1.0.2 follow-on requiring a different
//! audio crate (`cpal` + custom mixer).

use std::cell::RefCell;
use std::collections::HashMap;

/// Default window over which pool max-voices is enforced. Sounds
/// that play within this window count toward the cap.
pub const POOL_WINDOW_SECS: f64 = 1.0;

/// Default ramp rate for music layer weight changes — units of
/// weight per second. 4.0 ⇒ a 0→1 change takes 0.25s.
pub const LAYER_RAMP_RATE: f64 = 4.0;

#[derive(Clone)]
pub struct PoolConfig {
    pub max_voices: u32,
    /// Wall-clock timestamps of recent plays, in seconds since
    /// runtime start. Pruned each tick to the active window.
    pub recent: Vec<f64>,
}

#[derive(Clone)]
pub struct DuckEntry {
    pub channel: String,
    /// Volume scale factor applied to pooled plays in this channel
    /// while `while_playing` is sounding (in the layer set or
    /// recent plays). 0.0 = silent, 1.0 = unchanged.
    pub factor: f32,
    pub while_playing: String,
}

#[derive(Clone)]
pub struct LayerState {
    pub current_weight: f32,
    pub target_weight: f32,
    /// True until the layer has been instructed to actually `play`
    /// via macroquad; set in `play.rs::run_loop*` after sync. Stays
    /// true between frames so a re-call to `music.layer` with the
    /// same path doesn't restart the track.
    pub playing: bool,
}

#[derive(Clone)]
pub struct Crossfade {
    pub from_path: String,
    pub to_path: String,
    pub t: f64,
    pub duration: f64,
}

pub struct AudioPolishState {
    pub pools: HashMap<String, PoolConfig>,
    pub ducks: Vec<DuckEntry>,
    pub layers: HashMap<String, LayerState>,
    pub crossfades: Vec<Crossfade>,
    pub wall_clock: f64,
}

thread_local! {
    static AUDIO_POLISH: RefCell<AudioPolishState> = RefCell::new(AudioPolishState {
        pools: HashMap::new(),
        ducks: Vec::new(),
        layers: HashMap::new(),
        crossfades: Vec::new(),
        wall_clock: 0.0,
    });
}

/// Register / overwrite a pool for `path`. A `max_voices` of 0 means
/// "no throttling" — same effect as never calling `pool` for the path.
pub fn pool(path: &str, max_voices: u32) {
    AUDIO_POLISH.with(|s| {
        s.borrow_mut().pools.insert(
            path.to_string(),
            PoolConfig {
                max_voices,
                recent: Vec::new(),
            },
        );
    });
}

/// Add a duck entry. Subsequent pool plays in `channel` are scaled
/// by `factor` while a `while_playing` sound is sounding.
pub fn duck(channel: &str, factor: f32, while_playing: &str) {
    AUDIO_POLISH.with(|s| {
        s.borrow_mut().ducks.push(DuckEntry {
            channel: channel.to_string(),
            factor: factor.clamp(0.0, 1.0),
            while_playing: while_playing.to_string(),
        });
    });
}

/// Set the target weight of `path` as a music layer. Re-calling with
/// the same path re-targets the weight without restarting the track.
pub fn set_layer(path: &str, weight: f32) {
    AUDIO_POLISH.with(|s| {
        let mut st = s.borrow_mut();
        st.layers
            .entry(path.to_string())
            .and_modify(|l| l.target_weight = weight.clamp(0.0, 1.0))
            .or_insert(LayerState {
                current_weight: 0.0,
                target_weight: weight.clamp(0.0, 1.0),
                playing: false,
            });
    });
}

/// Start a crossfade from `from` to `to` over `duration` seconds.
/// Multiple concurrent crossfades are allowed; they each tick
/// independently.
pub fn crossfade(from: &str, to: &str, duration: f64) {
    AUDIO_POLISH.with(|s| {
        s.borrow_mut().crossfades.push(Crossfade {
            from_path: from.to_string(),
            to_path: to.to_string(),
            t: 0.0,
            duration: duration.max(0.001),
        });
    });
}

/// Check pool budget for `path` and, if allowed, record this play.
/// Returns `true` if the play should proceed.
pub fn try_pool_play(path: &str) -> bool {
    AUDIO_POLISH.with(|s| {
        let mut st = s.borrow_mut();
        let wc = st.wall_clock;
        let window = wc - POOL_WINDOW_SECS;
        if let Some(p) = st.pools.get_mut(path) {
            p.recent.retain(|&t| t >= window);
            if p.recent.len() as u32 >= p.max_voices && p.max_voices > 0 {
                return false;
            }
            p.recent.push(wc);
        }
        true
    })
}

/// Apply duck factor to `base_vol`, given a `channel`. Looks at the
/// duck table and any `while_playing` source that's currently in
/// the layer set (target_weight > 0) or in a recent pool play.
pub fn duck_scale(channel: &str, base_vol: f32) -> f32 {
    AUDIO_POLISH.with(|s| {
        let st = s.borrow();
        let wc = st.wall_clock;
        let window = wc - POOL_WINDOW_SECS;
        let mut factor: f32 = 1.0;
        for d in &st.ducks {
            if d.channel != channel {
                continue;
            }
            // "Currently sounding" = either a layer with positive
            // weight, or a recent pool play within the window.
            let layer_sounding = st
                .layers
                .get(&d.while_playing)
                .map(|l| l.current_weight > 0.001)
                .unwrap_or(false);
            let pool_sounding = st
                .pools
                .get(&d.while_playing)
                .map(|p| p.recent.iter().any(|&t| t >= window))
                .unwrap_or(false);
            if layer_sounding || pool_sounding {
                factor = factor.min(d.factor);
            }
        }
        base_vol * factor
    })
}

/// Advance wall clock + layer ramps + crossfade progress. Called once
/// per render frame from the play loop.
pub fn tick(dt: f64) {
    AUDIO_POLISH.with(|s| {
        let mut st = s.borrow_mut();
        st.wall_clock += dt;
        let rate = (LAYER_RAMP_RATE * dt) as f32;
        for layer in st.layers.values_mut() {
            let diff = layer.target_weight - layer.current_weight;
            if diff.abs() <= rate {
                // Snap to target when remaining gap fits in one step.
                layer.current_weight = layer.target_weight;
            } else {
                layer.current_weight += rate * diff.signum();
            }
        }
        for cf in &mut st.crossfades {
            cf.t += dt;
        }
        st.crossfades.retain(|cf| cf.t < cf.duration);
    });
}

/// Snapshot of layers whose `current_weight` changed this frame and
/// should be (re-)played / re-volumed at the macroquad level. The
/// caller marks `playing = true` on its first sync so a re-tick of
/// a layer with the same weight isn't a no-op restart.
pub fn dirty_layers() -> Vec<(String, f32, bool)> {
    AUDIO_POLISH.with(|s| {
        let st = s.borrow();
        st.layers
            .iter()
            .map(|(p, l)| (p.clone(), l.current_weight, l.playing))
            .collect()
    })
}

/// Mark `path` as playing — called by the play loop after it has
/// dispatched the macroquad `play_sound` for a layer for the first
/// time, so subsequent ticks just adjust volume.
pub fn mark_playing(path: &str) {
    AUDIO_POLISH.with(|s| {
        if let Some(l) = s.borrow_mut().layers.get_mut(path) {
            l.playing = true;
        }
    });
}

/// Snapshot of active crossfades: each entry is
/// `(from_path, to_path, from_volume, to_volume, finished)`.
/// `from_volume` ramps 1→0; `to_volume` ramps 0→1.
pub fn crossfade_snapshot() -> Vec<(String, String, f32, f32, bool)> {
    AUDIO_POLISH.with(|s| {
        let st = s.borrow();
        st.crossfades
            .iter()
            .map(|cf| {
                let t = (cf.t / cf.duration).clamp(0.0, 1.0) as f32;
                let from_v = (1.0 - t).max(0.0);
                let to_v = t.min(1.0);
                let finished = cf.t >= cf.duration;
                (
                    cf.from_path.clone(),
                    cf.to_path.clone(),
                    from_v,
                    to_v,
                    finished,
                )
            })
            .collect()
    })
}

/// Wipe all state. Called by `clear_asset_caches` on hot reload.
pub fn clear() {
    AUDIO_POLISH.with(|s| {
        *s.borrow_mut() = AudioPolishState {
            pools: HashMap::new(),
            ducks: Vec::new(),
            layers: HashMap::new(),
            crossfades: Vec::new(),
            wall_clock: 0.0,
        };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        clear();
    }

    #[test]
    fn pool_caps_within_window() {
        reset();
        pool("sfx/hit.wav", 3);
        // First three plays go through.
        assert!(try_pool_play("sfx/hit.wav"));
        assert!(try_pool_play("sfx/hit.wav"));
        assert!(try_pool_play("sfx/hit.wav"));
        // Fourth in the same window is throttled.
        assert!(!try_pool_play("sfx/hit.wav"));
    }

    #[test]
    fn pool_recovers_after_window() {
        reset();
        pool("hit.wav", 2);
        assert!(try_pool_play("hit.wav"));
        assert!(try_pool_play("hit.wav"));
        assert!(!try_pool_play("hit.wav"));
        // Advance past the window — old entries fall out.
        tick(POOL_WINDOW_SECS + 0.1);
        assert!(try_pool_play("hit.wav"));
    }

    #[test]
    fn unpooled_paths_play_freely() {
        reset();
        // No pool registered for "music.ogg" — always allowed.
        for _ in 0..20 {
            assert!(try_pool_play("music.ogg"));
        }
    }

    #[test]
    fn duck_scales_volume_when_layer_active() {
        reset();
        duck("sfx", 0.3, "boss_intro.ogg");
        // No layer yet — full volume.
        assert!((duck_scale("sfx", 1.0) - 1.0).abs() < 1e-6);
        // Layer with positive weight — ducked.
        set_layer("boss_intro.ogg", 1.0);
        // Layer current_weight starts at 0; needs a tick to ramp up.
        tick(1.0);
        let v = duck_scale("sfx", 1.0);
        assert!(
            (v - 0.3).abs() < 1e-6,
            "expected duck to 0.3, got {v}"
        );
    }

    #[test]
    fn duck_does_not_apply_when_silent() {
        reset();
        duck("sfx", 0.3, "boss_intro.ogg");
        set_layer("boss_intro.ogg", 0.0);
        // Even after a tick, layer weight stays 0 → no duck.
        tick(1.0);
        assert!((duck_scale("sfx", 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn layer_ramps_toward_target() {
        reset();
        set_layer("drums.ogg", 1.0);
        let start = dirty_layers()
            .iter()
            .find(|(p, ..)| p == "drums.ogg")
            .map(|(_, v, _)| *v)
            .unwrap();
        assert_eq!(start, 0.0);
        tick(0.5);
        let after = dirty_layers()
            .iter()
            .find(|(p, ..)| p == "drums.ogg")
            .map(|(_, v, _)| *v)
            .unwrap();
        assert!(after > 0.5 && after <= 1.0, "after = {after}");
        // Long enough to reach the target.
        tick(2.0);
        let done = dirty_layers()
            .iter()
            .find(|(p, ..)| p == "drums.ogg")
            .map(|(_, v, _)| *v)
            .unwrap();
        assert!((done - 1.0).abs() < 1e-6, "done = {done}");
    }

    #[test]
    fn crossfade_progress_then_finishes() {
        reset();
        crossfade("a.ogg", "b.ogg", 1.0);
        let snap = crossfade_snapshot();
        assert_eq!(snap.len(), 1);
        let (_, _, f, t, finished) = &snap[0];
        assert!((*f - 1.0).abs() < 1e-6);
        assert!(t.abs() < 1e-6);
        assert!(!*finished);
        tick(0.5);
        let snap2 = crossfade_snapshot();
        let (_, _, f2, t2, _) = &snap2[0];
        assert!((*f2 - 0.5).abs() < 1e-3);
        assert!((*t2 - 0.5).abs() < 1e-3);
        tick(0.6); // overshoot
        let snap3 = crossfade_snapshot();
        assert!(snap3.is_empty(), "crossfade should have finished");
    }
}
