//! Lanternfall G7: the game's sounds and its two music layers, synthesised.
//!
//! Writes 16-bit mono WAVs into `examples/lanternfall/assets/sfx/` if
//! they're missing, then checks each one matches its recipe, like
//! `gen_survive3d_sounds.rs`. The files are committed, so players never
//! run this; it is the procedural spec of the sounds (edit a recipe,
//! delete the file, rerun to regenerate). Deterministic: a fixed-seed
//! LCG drives every noise.
//!
//! The two music loops (`music_calm`, `music_tension`) are the same 12 s
//! long at the same tempo, so `music.layer` can blend them; every note's
//! tail wraps around the loop's end, so they loop without a seam.

use std::f32::consts::TAU;
use std::path::Path;

const RATE: u32 = 22_050;
const R: f32 = RATE as f32;

struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

fn samples(seconds: f32) -> usize {
    (seconds * R) as usize
}

fn sine(ph: f32) -> f32 {
    (ph * TAU).sin()
}

fn saw(ph: f32) -> f32 {
    ph * 2.0 - 1.0
}

fn square(ph: f32) -> f32 {
    if ph < 0.5 {
        1.0
    } else {
        -1.0
    }
}

fn tri(ph: f32) -> f32 {
    1.0 - 4.0 * (ph - 0.5).abs()
}

/// A note `semitones` above A4 (440 Hz).
fn note(semitones: f32) -> f32 {
    440.0 * 2f32.powf(semitones / 12.0)
}

/// An oscillator: frequency and amplitude as functions of time; the
/// phase accumulates, so sweeps are smooth.
fn osc(
    seconds: f32,
    hz: impl Fn(f32) -> f32,
    shape: fn(f32) -> f32,
    amp: impl Fn(f32) -> f32,
) -> Vec<f32> {
    let mut ph = 0.0f32;
    (0..samples(seconds))
        .map(|i| {
            let t = i as f32 / R;
            let v = shape(ph) * amp(t);
            ph = (ph + hz(t) / R).fract();
            v
        })
        .collect()
}

/// One-pole low-pass coefficient for a cutoff in Hz.
fn coef(hz: f32) -> f32 {
    1.0 - (-TAU * hz.max(1.0) / R).exp()
}

/// White noise through a low-pass with a cutoff that moves over time,
/// and a fixed high-pass (`hp` Hz, 0 for none).
fn hiss(
    seconds: f32,
    seed: u32,
    lp: impl Fn(f32) -> f32,
    hp: f32,
    amp: impl Fn(f32) -> f32,
) -> Vec<f32> {
    let mut n = Noise(seed);
    let mut low = 0.0f32;
    let mut floor = 0.0f32;
    let k_hp = coef(hp);
    (0..samples(seconds))
        .map(|i| {
            let t = i as f32 / R;
            low += coef(lp(t)) * (n.next() - low);
            let mut v = low;
            if hp > 0.0 {
                floor += k_hp * (v - floor);
                v -= floor;
            }
            v * amp(t)
        })
        .collect()
}

/// A low-pass whose cutoff moves over time.
fn lowpass(v: &[f32], cutoff: impl Fn(f32) -> f32) -> Vec<f32> {
    let mut y = 0.0f32;
    v.iter()
        .enumerate()
        .map(|(i, x)| {
            y += coef(cutoff(i as f32 / R)) * (x - y);
            y
        })
        .collect()
}

fn decay(rate: f32) -> impl Fn(f32) -> f32 {
    move |t| (-t * rate).exp()
}

/// A bell: a sine and two inharmonic partials, each fading faster.
fn bell(hz: f32, seconds: f32, rate: f32) -> Vec<f32> {
    let mut b = Buf::new(seconds);
    b.add(0.0, &osc(seconds, |_| hz, sine, decay(rate)), 1.0);
    b.add(
        0.0,
        &osc(seconds, |_| hz * 2.76, sine, decay(rate * 1.8)),
        0.35,
    );
    b.add(
        0.0,
        &osc(seconds, |_| hz * 5.4, sine, decay(rate * 3.0)),
        0.12,
    );
    b.s
}

/// A plucked string-ish note: a saw whose brightness fades with it.
fn pluck(hz: f32, seconds: f32, rate: f32, bright: f32) -> Vec<f32> {
    let raw = osc(seconds, |_| hz, saw, decay(rate));
    lowpass(&raw, |t| 180.0 + bright * (-t * rate * 2.0).exp())
}

/// A buffer that sounds are mixed into, at times in seconds. A looped
/// buffer wraps whatever runs past its end back to its start.
struct Buf {
    s: Vec<f32>,
    wrap: bool,
}

impl Buf {
    fn new(seconds: f32) -> Self {
        Buf {
            s: vec![0.0; samples(seconds)],
            wrap: false,
        }
    }

    fn looped(seconds: f32) -> Self {
        Buf {
            s: vec![0.0; samples(seconds)],
            wrap: true,
        }
    }

    fn add(&mut self, at: f32, v: &[f32], gain: f32) {
        let n = self.s.len();
        let start = (at * R).round() as usize;
        for (i, x) in v.iter().enumerate() {
            let j = start + i;
            if self.wrap {
                self.s[j % n] += x * gain;
            } else if j < n {
                self.s[j] += x * gain;
            }
        }
    }
}

/// Scale to `peak`; one-shots get a 2 ms fade in and 5 ms fade out so
/// nothing clicks.
fn finish(mut v: Vec<f32>, peak: f32, one_shot: bool) -> Vec<f32> {
    if one_shot {
        let n = v.len();
        let fin = samples(0.002).max(1);
        let fout = samples(0.005).max(1).min(n);
        for (i, x) in v.iter_mut().enumerate() {
            if i < fin {
                *x *= i as f32 / fin as f32;
            }
            if i + fout >= n {
                *x *= (n - i) as f32 / fout as f32;
            }
        }
    }
    let max = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if max > 0.0 {
        for x in &mut v {
            *x *= peak / max;
        }
    }
    v
}

/// A gunshot: a bright crack, a noise body and a falling thump.
fn shot(
    seconds: f32,
    seed: u32,
    body_lp: f32,
    body_rate: f32,
    thump: (f32, f32),
    crack: f32,
) -> Vec<f32> {
    let mut b = Buf::new(seconds);
    b.add(
        0.0,
        &hiss(0.03, seed, |_| 9000.0, 2000.0, decay(120.0)),
        crack,
    );
    b.add(
        0.0,
        &hiss(
            seconds,
            seed + 1,
            |t| body_lp * (-t * 6.0).exp() + 200.0,
            60.0,
            decay(body_rate),
        ),
        1.0,
    );
    let (from, to) = thump;
    b.add(
        0.0,
        &osc(
            seconds,
            |t| to + (from - to) * (-t * 30.0).exp(),
            sine,
            decay(body_rate * 0.8),
        ),
        0.7,
    );
    b.s
}

fn sounds() -> Vec<(&'static str, Vec<f32>)> {
    let mut out = Vec::new();

    // Guns.
    out.push((
        "pistol",
        finish(shot(0.25, 11, 4000.0, 26.0, (190.0, 80.0), 0.6), 0.7, true),
    ));
    out.push((
        "smg",
        finish(shot(0.13, 12, 5500.0, 42.0, (260.0, 140.0), 0.5), 0.5, true),
    ));
    out.push((
        "scatter",
        finish(shot(0.5, 13, 2600.0, 9.0, (110.0, 42.0), 0.8), 0.85, true),
    ));
    let rifle = {
        let dry = shot(0.75, 14, 3500.0, 7.0, (150.0, 55.0), 1.2);
        let mut b = Buf::new(0.75);
        b.add(0.0, &dry, 1.0);
        b.add(0.13, &dry, 0.28);
        b.s
    };
    out.push(("rifle", finish(rifle, 0.85, true)));
    let mortar = {
        let mut b = Buf::new(0.4);
        b.add(
            0.0,
            &osc(0.4, |t| 55.0 + 110.0 * (-t * 14.0).exp(), sine, decay(9.0)),
            1.0,
        );
        b.add(0.0, &hiss(0.4, 15, |_| 700.0, 40.0, decay(22.0)), 0.6);
        b.s
    };
    out.push(("mortar", finish(mortar, 0.7, true)));

    // Hits.
    let swing = hiss(
        0.26,
        16,
        |t| 600.0 + 3000.0 * (t / 0.26 * std::f32::consts::PI).sin(),
        300.0,
        |t| (t / 0.26 * std::f32::consts::PI).sin().powi(2),
    );
    out.push(("swing", finish(swing, 0.5, true)));
    let hit = {
        let mut b = Buf::new(0.1);
        b.add(
            0.0,
            &osc(
                0.1,
                |t| 110.0 + 130.0 * (-t * 40.0).exp(),
                sine,
                decay(40.0),
            ),
            1.0,
        );
        b.add(0.0, &hiss(0.03, 17, |_| 5000.0, 800.0, decay(150.0)), 0.5);
        b.s
    };
    out.push(("hit", finish(hit, 0.55, true)));
    let ping = {
        let mut b = Buf::new(0.16);
        b.add(0.0, &osc(0.16, |_| 1850.0, sine, decay(28.0)), 0.6);
        b.add(0.0, &osc(0.16, |_| 2710.0, sine, decay(36.0)), 0.4);
        b.add(0.0, &hiss(0.02, 18, |_| 8000.0, 3000.0, decay(200.0)), 0.4);
        b.s
    };
    out.push(("shield_hit", finish(ping, 0.4, true)));
    let crack = {
        let mut b = Buf::new(0.6);
        let mut n = Noise(19);
        for _ in 0..18 {
            let at = (n.next() * 0.5 + 0.5) * 0.12;
            let hz = 2200.0 + (n.next() * 0.5 + 0.5) * 3800.0;
            b.add(at, &osc(0.3, move |_| hz, sine, decay(22.0)), 0.18);
        }
        b.add(0.0, &hiss(0.6, 20, |_| 9000.0, 2500.0, decay(9.0)), 0.6);
        b.s
    };
    out.push(("crack", finish(crack, 0.6, true)));
    let boom = {
        let mut b = Buf::new(1.5);
        b.add(0.0, &hiss(0.04, 21, |_| 8000.0, 1500.0, decay(90.0)), 0.8);
        b.add(
            0.0,
            &hiss(
                1.5,
                22,
                |t| 120.0 + 2500.0 * (-t * 4.0).exp(),
                20.0,
                decay(3.2),
            ),
            1.0,
        );
        b.add(
            0.0,
            &osc(1.5, |t| 30.0 + 45.0 * (-t * 5.0).exp(), sine, decay(3.5)),
            0.9,
        );
        b.s
    };
    out.push(("boom", finish(boom, 0.95, true)));
    let starcall = {
        let mut b = Buf::new(1.2);
        b.add(
            0.0,
            &osc(
                1.2,
                |t| 300.0 + 1300.0 * (t / 1.2).powi(2) + 12.0 * (t * 38.0).sin(),
                sine,
                |t| (t / 1.2).powi(2),
            ),
            1.0,
        );
        b.add(
            0.0,
            &osc(
                1.2,
                |t| 450.0 + 1950.0 * (t / 1.2).powi(2),
                tri,
                |t| (t / 1.2).powi(3),
            ),
            0.35,
        );
        b.add(
            0.0,
            &hiss(1.2, 23, |_| 7000.0, 3000.0, |t| (t / 1.2).powi(2)),
            0.25,
        );
        b.s
    };
    out.push(("starcall", finish(starcall, 0.5, true)));

    // Deaths.
    let kill = {
        let mut b = Buf::new(0.7);
        b.add(0.0, &bell(note(7.0), 0.7, 7.0), 1.0);
        b.add(0.08, &bell(note(14.0), 0.62, 6.0), 0.9);
        b.s
    };
    out.push(("kill", finish(kill, 0.55, true)));
    let down = {
        let mut b = Buf::new(0.6);
        b.add(
            0.0,
            &hiss(
                0.6,
                24,
                |t| 200.0 + 2500.0 * (-t * 6.0).exp(),
                40.0,
                decay(6.0),
            ),
            1.0,
        );
        b.add(
            0.0,
            &osc(0.6, |t| 80.0 + 320.0 * (-t * 7.0).exp(), tri, decay(6.0)),
            0.6,
        );
        b.s
    };
    out.push(("down", finish(down, 0.5, true)));

    // Loot.
    let chest = {
        let mut b = Buf::new(0.75);
        let creak = osc(
            0.35,
            |t| 95.0 + 35.0 * (t * 23.0).sin() + 60.0 * t,
            saw,
            |t| (t / 0.35 * std::f32::consts::PI).sin(),
        );
        b.add(0.0, &lowpass(&creak, |_| 900.0), 0.6);
        let mut n = Noise(25);
        for k in 0..7 {
            let hz = note(15.0 + (n.next() * 0.5 + 0.5) * 12.0);
            b.add(0.3 + k as f32 * 0.05, &bell(hz, 0.4, 9.0), 0.2);
        }
        b.s
    };
    out.push(("chest", finish(chest, 0.55, true)));
    let pickup = {
        let mut b = Buf::new(0.16);
        b.add(0.0, &osc(0.07, |_| note(12.0), sine, decay(30.0)), 1.0);
        b.add(0.06, &osc(0.1, |_| note(19.0), sine, decay(28.0)), 1.0);
        b.s
    };
    out.push(("pickup", finish(pickup, 0.4, true)));
    let clack = |seed: u32| {
        let mut b = Buf::new(0.08);
        b.add(0.0, &hiss(0.08, seed, |_| 6000.0, 1500.0, decay(90.0)), 0.6);
        b.add(0.0, &osc(0.08, |_| 1200.0, square, decay(80.0)), 0.25);
        b.s
    };
    let gun = {
        let mut b = Buf::new(0.32);
        b.add(0.0, &clack(26), 1.0);
        b.add(0.14, &clack(27), 1.2);
        b.s
    };
    out.push(("gun", finish(gun, 0.5, true)));
    let reload = {
        let mut b = Buf::new(0.55);
        b.add(0.0, &clack(28), 0.8);
        b.add(
            0.12,
            &hiss(
                0.15,
                29,
                |_| 2500.0,
                600.0,
                |t| (t / 0.15 * std::f32::consts::PI).sin(),
            ),
            0.4,
        );
        b.add(0.35, &clack(30), 1.2);
        b.add(0.42, &clack(31), 0.7);
        b.s
    };
    out.push(("reload", finish(reload, 0.45, true)));
    out.push(("empty", finish(clack(32), 0.35, true)));

    // Moving.
    let whoosh = |seconds: f32, seed: u32, rise: f32| {
        hiss(
            seconds,
            seed,
            move |t| 400.0 + rise * (t / seconds),
            150.0,
            move |t| (t / seconds * std::f32::consts::PI).sin().powi(2),
        )
    };
    out.push(("leap", finish(whoosh(0.55, 33, 3500.0), 0.45, true)));
    out.push(("roll", finish(whoosh(0.3, 34, 1800.0), 0.35, true)));
    let land = {
        let mut b = Buf::new(0.35);
        b.add(
            0.0,
            &osc(0.35, |t| 45.0 + 60.0 * (-t * 20.0).exp(), sine, decay(14.0)),
            1.0,
        );
        b.add(0.0, &hiss(0.35, 35, |_| 900.0, 60.0, decay(14.0)), 0.6);
        b.s
    };
    out.push(("land", finish(land, 0.6, true)));

    // Shields.
    let charge = {
        let mut b = Buf::new(2.5);
        let tremolo = |t: f32| (0.6 + 0.4 * (t * TAU * 7.0).sin()) * (t / 2.5).min(1.0);
        b.add(
            0.0,
            &osc(2.5, |t| 220.0 + 220.0 * t / 2.5, sine, tremolo),
            1.0,
        );
        b.add(
            0.0,
            &osc(2.5, |t| 330.0 + 330.0 * t / 2.5, tri, tremolo),
            0.3,
        );
        b.s
    };
    out.push(("charge", finish(charge, 0.32, true)));
    let shield_up = {
        let mut b = Buf::new(0.7);
        for (k, n) in [15.0, 19.0, 22.0, 27.0].iter().enumerate() {
            b.add(k as f32 * 0.06, &bell(note(*n), 0.5, 7.0), 0.7);
        }
        b.s
    };
    out.push(("shield_up", finish(shield_up, 0.45, true)));

    // The round.
    let banner = {
        let mut b = Buf::new(1.4);
        let swell = |t: f32| (t / 0.02).min(1.0) * (-t * 2.6).exp();
        b.add(0.0, &osc(1.4, |_| 110.0, sine, swell), 1.0);
        b.add(0.0, &osc(1.4, |_| 221.5, sine, swell), 0.5);
        b.add(0.0, &osc(1.4, |_| 329.0, sine, swell), 0.3);
        b.add(0.0, &osc(1.4, |_| 523.0, sine, decay(4.0)), 0.15);
        b.s
    };
    out.push(("banner", finish(banner, 0.5, true)));
    let victory = {
        let mut b = Buf::new(2.4);
        for (k, n) in [0.0f32, 4.0, 7.0, 12.0].iter().enumerate() {
            b.add(k as f32 * 0.13, &bell(note(*n), 1.6, 3.0), 0.6);
        }
        for n in [0.0f32, 4.0, 7.0] {
            b.add(
                0.55,
                &osc(
                    1.85,
                    move |_| note(n - 12.0),
                    tri,
                    |t| (t / 0.1).min(1.0) * (-t * 1.6).exp(),
                ),
                0.25,
            );
        }
        b.s
    };
    out.push(("victory", finish(victory, 0.55, true)));
    let defeat = {
        let mut b = Buf::new(2.0);
        for (k, n) in [0.0f32, -4.0, -7.0, -12.0].iter().enumerate() {
            b.add(
                k as f32 * 0.28,
                &pluck(note(*n - 12.0), 1.2, 2.5, 1500.0),
                0.6,
            );
        }
        b.s
    };
    out.push(("defeat", finish(defeat, 0.45, true)));
    let heartbeat = {
        let mut b = Buf::new(0.5);
        let thump = osc(0.2, |t| 40.0 + 30.0 * (-t * 25.0).exp(), sine, decay(18.0));
        b.add(0.0, &thump, 1.0);
        b.add(0.17, &thump, 0.7);
        b.s
    };
    out.push(("heartbeat", finish(heartbeat, 0.6, true)));
    let hush = {
        let mut b = Buf::new(0.28);
        let buzz = osc(
            0.28,
            |t| 68.0 + 8.0 * (t * 30.0).sin(),
            square,
            |t| (t / 0.28 * std::f32::consts::PI).sin(),
        );
        b.add(0.0, &lowpass(&buzz, |_| 600.0), 1.0);
        b.add(
            0.0,
            &hiss(
                0.28,
                36,
                |_| 1500.0,
                300.0,
                |t| (t / 0.28 * std::f32::consts::PI).sin(),
            ),
            0.4,
        );
        b.s
    };
    out.push(("hush", finish(hush, 0.4, true)));

    // The menus.
    out.push((
        "click",
        finish(
            osc(0.05, |t| 1400.0 - 6000.0 * t, sine, decay(70.0)),
            0.35,
            true,
        ),
    ));
    out.push((
        "tick",
        finish(osc(0.03, |_| 2200.0, sine, decay(120.0)), 0.2, true),
    ));

    out.push(("music_calm", finish(music_calm(), 0.55, false)));
    out.push(("music_tension", finish(music_tension(), 0.6, false)));
    out
}

const LOOP: f32 = 12.0;
const BAR: f32 = 3.0;
const EIGHTH: f32 = 0.375;

/// Night music: a slow pad (Am – F – C – G, voices moving little), a low
/// root under it, bell figures over it and a breath of wind.
fn music_calm() -> Vec<f32> {
    let mut b = Buf::looped(LOOP);
    // (pad voices, bass) in semitones from A4.
    let chords: [([f32; 3], f32); 4] = [
        ([-12.0, -9.0, -5.0], -24.0),
        ([-12.0, -9.0, -4.0], -28.0),
        ([-14.0, -9.0, -5.0], -21.0),
        ([-14.0, -10.0, -7.0], -26.0),
    ];
    let swell = |t: f32| {
        let a = (t / 0.9).min(1.0);
        let r = ((3.8 - t) / 0.9).clamp(0.0, 1.0);
        let e = a.min(r);
        e * e * (3.0 - 2.0 * e)
    };
    for (bar, (voices, bass)) in chords.iter().enumerate() {
        let at = (bar as f32 * BAR - 0.4).rem_euclid(LOOP);
        for v in voices {
            let hz = note(*v);
            let mut pad = osc(3.8, move |_| hz * 0.9985, sine, swell);
            for (x, y) in pad
                .iter_mut()
                .zip(osc(3.8, move |_| hz * 1.0015, sine, swell))
            {
                *x += y;
            }
            for (x, y) in pad.iter_mut().zip(osc(3.8, move |_| hz * 2.0, tri, swell)) {
                *x += y * 0.2;
            }
            b.add(at, &lowpass(&pad, |_| 1400.0), 0.11);
        }
        let root = note(*bass);
        b.add(at, &osc(3.8, move |_| root, sine, swell), 0.2);
        b.add(at, &osc(3.8, move |_| root * 2.0, sine, swell), 0.06);
    }
    // Bells: chord tones an octave up; -1 rests.
    let tones: [[f32; 4]; 4] = [
        [0.0, 3.0, 7.0, 12.0],
        [-4.0, 0.0, 3.0, 8.0],
        [3.0, 7.0, 10.0, 15.0],
        [-2.0, 2.0, 5.0, 10.0],
    ];
    let figures: [[i32; 8]; 4] = [
        [0, -1, 2, -1, 3, -1, 2, -1],
        [-1, 1, -1, 2, -1, 0, -1, -1],
        [0, -1, 1, 2, -1, -1, 3, -1],
        [-1, 2, -1, 1, -1, 0, -1, -1],
    ];
    for (bar, (figure, chord)) in figures.iter().zip(tones.iter()).enumerate() {
        for (step, &i) in figure.iter().enumerate() {
            if i >= 0 {
                let hz = note(chord[i as usize]);
                b.add(
                    bar as f32 * BAR + step as f32 * EIGHTH,
                    &bell(hz, 1.8, 3.2),
                    0.075,
                );
            }
        }
    }
    let wind = hiss(
        LOOP,
        40,
        |_| 450.0,
        80.0,
        |t| 0.6 + 0.4 * (t / LOOP * TAU).sin(),
    );
    b.add(0.0, &wind, 0.05);
    b.s
}

/// The Hush closing in: a drone on A, a pulsing bass, a heartbeat kick,
/// ticking hats and a low drum at each bar. Pitches are A and E only, so
/// it sits under the calm layer whatever their alignment.
fn music_tension() -> Vec<f32> {
    let mut b = Buf::looped(LOOP);
    // The drone runs a second before the loop so its filter has settled;
    // 110 Hz and 55 Hz fit whole cycles in 12 s, so it joins seamlessly.
    let pre = 1.0;
    let drone = osc(LOOP + pre, |_| 110.0, saw, |_| 1.0);
    let drone = lowpass(&drone, |_| 380.0);
    b.add(0.0, &drone[samples(pre)..], 0.07);
    b.add(0.0, &osc(LOOP, |_| 55.0, sine, |_| 1.0), 0.12);
    let pattern = [110.0f32, 110.0, 110.0, 82.41, 110.0, 110.0, 82.41, 110.0];
    for k in 0..32 {
        let hz = pattern[k % 8];
        let accent = if k % 2 == 0 { 1.0 } else { 0.6 };
        b.add(
            k as f32 * EIGHTH,
            &pluck(hz, 0.4, 7.0, 1700.0),
            0.16 * accent,
        );
    }
    let kick = osc(0.3, |t| 40.0 + 80.0 * (-t * 18.0).exp(), sine, decay(13.0));
    for beat in 0..16 {
        let gain = if beat % 2 == 0 { 0.5 } else { 0.28 };
        b.add(beat as f32 * 0.75, &kick, gain);
    }
    for k in 0..32 {
        if k % 2 == 1 {
            b.add(
                k as f32 * EIGHTH,
                &hiss(0.05, 41 + k as u32, |_| 9000.0, 5000.0, decay(70.0)),
                0.12,
            );
        }
    }
    for bar in 0..4 {
        let mut drum = osc(0.6, |t| 60.0 + 30.0 * (-t * 10.0).exp(), sine, decay(6.0));
        for (x, y) in drum
            .iter_mut()
            .zip(hiss(0.6, 50 + bar, |_| 350.0, 30.0, decay(8.0)))
        {
            *x += y * 0.7;
        }
        b.add(bar as f32 * BAR, &drum, 0.45);
    }
    b.s
}

fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}

#[test]
fn lanternfall_sounds_exist_and_match_their_recipes() {
    let dir = Path::new("examples/lanternfall/assets/sfx");
    std::fs::create_dir_all(dir).expect("sfx dir");
    for (name, s) in sounds() {
        let path = dir.join(format!("{name}.wav"));
        let bytes = wav(&s);
        if !path.exists() {
            std::fs::write(&path, &bytes).expect("write wav");
        }
        let on_disk = std::fs::read(&path).expect("read wav");
        assert!(
            on_disk.starts_with(b"RIFF") && &on_disk[8..12] == b"WAVE",
            "{name}: a WAV"
        );
        assert!(on_disk.len() > 44 + 200, "{name}: has samples");
        assert_eq!(on_disk, bytes, "{name}.wav is stale: delete it and rerun");
    }
}

#[test]
fn the_music_layers_share_a_length_and_loop_cleanly() {
    let all = sounds();
    let calm = &all.iter().find(|(n, _)| *n == "music_calm").unwrap().1;
    let tension = &all.iter().find(|(n, _)| *n == "music_tension").unwrap().1;
    assert_eq!(
        calm.len(),
        tension.len(),
        "same length, so the layers stay together"
    );
    for m in [calm, tension] {
        // The seam: the jump from the last sample to the first is no
        // bigger than the loop's ordinary sample-to-sample steps.
        let seam = (m[0] - m[m.len() - 1]).abs();
        let typical = m
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(seam <= typical, "seam {seam} vs largest step {typical}");
    }
}
