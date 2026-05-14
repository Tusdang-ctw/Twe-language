//! `tween.*` — deterministic easing primitives (v1.0.1 Session 2).
//!
//! Six pure functions. No thread_local, no `dt` accumulator, no
//! hidden global state. Replay-safe by construction: every output is
//! a pure function of its inputs, so byte-identical across runs.
//!
//! Why no registry: Twe has no refs, and the plan's stated principle
//! is "deterministic — eases are pure functions; no hidden global
//! state" (`docs/v1.0.1-plan.md` §Session 2). A registry would
//! couple to a tick-source and re-introduce the determinism worry
//! the principle exists to prevent. Scripts can build a registry
//! on top in seven lines if they want one; the language ships the
//! deterministic primitive.
//!
//! Ease names follow the Penner family. `linear` / `ease_in_quad` /
//! `ease_out_quad` / `ease_in_out_quad` / `ease_in_cubic` /
//! `ease_out_cubic` / `ease_in_out_cubic` / `ease_in_quart` /
//! `ease_out_quart` / `ease_in_out_quart` / `ease_out_back` /
//! `ease_out_elastic` / `ease_out_bounce` / `smoothstep` are all
//! supported. `smoothstep` is an alias for `ease_in_out_quad` (same
//! `3t² − 2t³` Hermite curve that the WGSL `smoothstep` builtin uses).

/// Apply the named easing curve to `t`. `t` is clamped to `[0, 1]`
/// so callers don't have to worry about overshoot from a slightly-
/// off accumulator. Returns `None` if the ease name is unknown so
/// the stdlib wrapper can surface a "did_you_mean" error.
pub fn ease(name: &str, t: f64) -> Option<f64> {
    let t = t.clamp(0.0, 1.0);
    let v = match name {
        "linear" => t,
        "ease_in_quad" => t * t,
        "ease_out_quad" => 1.0 - (1.0 - t) * (1.0 - t),
        "ease_in_out_quad" | "smoothstep" => {
            // Hermite 3t² − 2t³. Same curve as WGSL `smoothstep(0,1,t)`.
            t * t * (3.0 - 2.0 * t)
        }
        "ease_in_cubic" => t * t * t,
        "ease_out_cubic" => {
            let u = 1.0 - t;
            1.0 - u * u * u
        }
        "ease_in_out_cubic" => {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                let u = -2.0 * t + 2.0;
                1.0 - u * u * u / 2.0
            }
        }
        "ease_in_quart" => t * t * t * t,
        "ease_out_quart" => {
            let u = 1.0 - t;
            1.0 - u * u * u * u
        }
        "ease_in_out_quart" => {
            if t < 0.5 {
                8.0 * t * t * t * t
            } else {
                let u = -2.0 * t + 2.0;
                1.0 - u * u * u * u / 2.0
            }
        }
        "ease_out_back" => {
            // Penner-classic overshoot constants.
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            let u = t - 1.0;
            1.0 + c3 * u * u * u + c1 * u * u
        }
        "ease_out_elastic" => {
            // Decaying sine. Clamped to (0, 1) explicit endpoints.
            if t == 0.0 || t == 1.0 {
                t
            } else {
                let c4 = std::f64::consts::TAU / 3.0;
                (2.0_f64).powf(-10.0 * t) * ((t * 10.0 - 0.75) * c4).sin() + 1.0
            }
        }
        "ease_out_bounce" => ease_out_bounce(t),
        _ => return None,
    };
    Some(v)
}

fn ease_out_bounce(t: f64) -> f64 {
    let n1 = 7.5625;
    let d1 = 2.75;
    if t < 1.0 / d1 {
        n1 * t * t
    } else if t < 2.0 / d1 {
        let u = t - 1.5 / d1;
        n1 * u * u + 0.75
    } else if t < 2.5 / d1 {
        let u = t - 2.25 / d1;
        n1 * u * u + 0.9375
    } else {
        let u = t - 2.625 / d1;
        n1 * u * u + 0.984_375
    }
}

/// Linear interpolation. Not strictly necessary (`a + (b-a)*t` is one
/// expression in Twe) but ships as a builtin for P2 (one obvious way
/// to lerp) and to keep ease-using code symmetric with `lerp_eased`.
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Eased linear interpolation. Equivalent to `lerp(a, b, ease(name, t))`.
/// Returns `None` for unknown ease names.
pub fn lerp_eased(a: f64, b: f64, t: f64, name: &str) -> Option<f64> {
    ease(name, t).map(|e| a + (b - a) * e)
}

/// Out-and-back bounce envelope over `t ∈ [0, 1]`. Peaks at the
/// midpoint and returns to `a` at the endpoints. Uses `4t(1-t)` —
/// the same quadratic the Phase 9 particles `velocity_curve` uses
/// for "puff" shapes — to keep the curve C¹-continuous.
///
/// `bounce_value(a, b, 0.0) == a`
/// `bounce_value(a, b, 0.5) == b`
/// `bounce_value(a, b, 1.0) == a`
pub fn bounce_value(a: f64, b: f64, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let env = 4.0 * t * (1.0 - t);
    a + (b - a) * env
}

/// Deterministic shake envelope. `t ∈ [0, 1]` is the normalised
/// elapsed time; `freq` is in cycles per `t`-unit; `seed` is added
/// as a phase offset so different shake instances don't sync. The
/// amplitude decays linearly to zero at `t = 1` — pass the return
/// to `lerp(base, base + amount, shake(...))` or just add it to
/// the base value directly.
///
/// Returns a value in `[-1, 1] · (1 - t)`.
pub fn shake(seed: f64, t: f64, freq: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    // Pin both endpoints to a positive zero. Without this the
    // `sin * (1 - t)` product yields IEEE `-0.0` when sin rounds
    // slightly negative at t = 1 — a real but cosmetic surprise
    // that's easier to eliminate here than to explain at every
    // print site.
    if t == 0.0 || t == 1.0 {
        return 0.0;
    }
    let decay = 1.0 - t;
    let phase = t * freq * std::f64::consts::TAU + seed;
    phase.sin() * decay
}

/// Canonical list of ease names. Exposed via `tween.eases()` for
/// `twec stdlib --json` (P4: AI-legible — an LLM doesn't have to
/// guess which Penner curves shipped).
pub const EASE_NAMES: &[&str] = &[
    "linear",
    "ease_in_quad",
    "ease_out_quad",
    "ease_in_out_quad",
    "ease_in_cubic",
    "ease_out_cubic",
    "ease_in_out_cubic",
    "ease_in_quart",
    "ease_out_quart",
    "ease_in_out_quart",
    "ease_out_back",
    "ease_out_elastic",
    "ease_out_bounce",
    "smoothstep",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "expected {b}, got {a}");
    }

    #[test]
    fn linear_endpoints_and_midpoint() {
        approx(ease("linear", 0.0).unwrap(), 0.0);
        approx(ease("linear", 0.5).unwrap(), 0.5);
        approx(ease("linear", 1.0).unwrap(), 1.0);
    }

    #[test]
    fn ease_endpoints_all_zero_one() {
        // Every Penner curve except elastic's special-cased
        // endpoints pins (0,0) and (1,1). Elastic also pins both
        // endpoints exactly via the if-clause in `ease`.
        for name in EASE_NAMES {
            approx(ease(name, 0.0).unwrap(), 0.0);
            approx(ease(name, 1.0).unwrap(), 1.0);
        }
    }

    #[test]
    fn ease_clamps_overshoot() {
        // t < 0 → t = 0, t > 1 → t = 1. No NaN, no out-of-range.
        approx(ease("ease_out_cubic", -0.5).unwrap(), 0.0);
        approx(ease("ease_out_cubic", 1.5).unwrap(), 1.0);
    }

    #[test]
    fn ease_out_cubic_known_values() {
        // 1 - (1-t)^3 at t=0.5 = 1 - 0.125 = 0.875
        approx(ease("ease_out_cubic", 0.5).unwrap(), 0.875);
    }

    #[test]
    fn smoothstep_matches_ease_in_out_quad() {
        for i in 0..=20 {
            let t = i as f64 / 20.0;
            approx(
                ease("smoothstep", t).unwrap(),
                ease("ease_in_out_quad", t).unwrap(),
            );
        }
    }

    #[test]
    fn unknown_ease_returns_none() {
        assert!(ease("ease_in_outt_quad", 0.5).is_none());
        assert!(ease("", 0.5).is_none());
    }

    #[test]
    fn lerp_simple() {
        approx(lerp(0.0, 10.0, 0.0), 0.0);
        approx(lerp(0.0, 10.0, 0.5), 5.0);
        approx(lerp(0.0, 10.0, 1.0), 10.0);
        approx(lerp(-1.0, 1.0, 0.25), -0.5);
    }

    #[test]
    fn lerp_eased_uses_named_curve() {
        // ease_in_quad at 0.5 is 0.25, so lerp 0..10 = 2.5.
        approx(lerp_eased(0.0, 10.0, 0.5, "ease_in_quad").unwrap(), 2.5);
    }

    #[test]
    fn bounce_value_peaks_at_midpoint() {
        approx(bounce_value(1.0, 1.15, 0.0), 1.0);
        approx(bounce_value(1.0, 1.15, 0.5), 1.15);
        approx(bounce_value(1.0, 1.15, 1.0), 1.0);
        // Quadratic at t=0.25: 4*0.25*0.75 = 0.75 of the way up.
        approx(bounce_value(0.0, 1.0, 0.25), 0.75);
    }

    #[test]
    fn shake_decays_to_zero_and_is_deterministic() {
        approx(shake(0.0, 1.0, 8.0), 0.0); // decay zeroes the endpoint
        let a = shake(2.0, 0.3, 8.0);
        let b = shake(2.0, 0.3, 8.0);
        approx(a, b); // pure function — same args, same result
        // Different seeds produce different values.
        let c = shake(0.0, 0.3, 8.0);
        assert!((a - c).abs() > 1e-9);
    }

    #[test]
    fn shake_bounded_by_decay_envelope() {
        // |shake(seed, t, freq)| <= 1 - t  for all valid t.
        for i in 0..=100 {
            let t = i as f64 / 100.0;
            let v = shake(1.23, t, 5.0).abs();
            assert!(v <= (1.0 - t) + 1e-12, "t={t} v={v}");
        }
    }
}
