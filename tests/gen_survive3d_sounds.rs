//! web3d-M4: the sound effects of `examples/survive3d`, synthesised.
//!
//! Generates small 16-bit mono WAVs into `examples/survive3d/assets/`
//! if they're missing, then checks each is a valid WAV. The files are
//! committed, so players never run this; it is the procedural spec of
//! the sounds (edit a recipe, delete the file, rerun to regenerate).
//! Deterministic: a fixed-seed LCG drives the noise.

use std::path::Path;

const RATE: u32 = 22_050;

/// A sound recipe: duration, and a sample function of (t seconds, its
/// progress 0..1, a noise source).
struct Recipe {
    name: &'static str,
    seconds: f32,
    sample: fn(f32, f32, &mut Noise) -> f32,
}

struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

fn square(t: f32, hz: f32) -> f32 {
    if (t * hz).fract() < 0.5 {
        1.0
    } else {
        -1.0
    }
}

fn sine(t: f32, hz: f32) -> f32 {
    (t * hz * std::f32::consts::TAU).sin()
}

const RECIPES: &[Recipe] = &[
    // A short falling zap: the auto-fired bolt.
    Recipe {
        name: "shot.wav",
        seconds: 0.08,
        sample: |t, p, _| square(t, 880.0 - 500.0 * p) * (1.0 - p) * 0.25,
    },
    // A noisy thump: an enemy dies.
    Recipe {
        name: "hit.wav",
        seconds: 0.12,
        sample: |t, p, n| {
            (n.next() * 0.6 + sine(t, 140.0 - 80.0 * p) * 0.6) * (1.0 - p).powi(2) * 0.5
        },
    },
    // A bright rising blip: an XP gem collected.
    Recipe {
        name: "pickup.wav",
        seconds: 0.07,
        sample: |t, p, _| sine(t, 1200.0 + 900.0 * p) * (1.0 - p) * 0.3,
    },
    // A low crunch: the player is hurt.
    Recipe {
        name: "hurt.wav",
        seconds: 0.2,
        sample: |t, p, n| (square(t, 110.0 - 50.0 * p) * 0.5 + n.next() * 0.5) * (1.0 - p) * 0.4,
    },
    // A three-note arpeggio: level up.
    Recipe {
        name: "levelup.wav",
        seconds: 0.36,
        sample: |t, p, _| {
            let hz = [523.25, 659.25, 783.99][((p * 3.0) as usize).min(2)];
            (sine(t, hz) * 0.7 + sine(t, hz * 2.0) * 0.2) * (1.0 - p * 0.6) * 0.35
        },
    },
    // A long falling rumble: the boss arrives.
    Recipe {
        name: "boss.wav",
        seconds: 0.9,
        sample: |t, p, n| (square(t, 70.0 - 30.0 * p) * 0.5 + n.next() * 0.3) * (1.0 - p) * 0.5,
    },
];

fn wav(samples: &[i16]) -> Vec<u8> {
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
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn render(r: &Recipe) -> Vec<u8> {
    let n = (r.seconds * RATE as f32) as usize;
    let mut noise = Noise(0x5eed_2026);
    let samples: Vec<i16> = (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let p = i as f32 / n as f32;
            // 3 ms fade in, so no click at the start.
            let fade = (t / 0.003).min(1.0);
            let v = ((r.sample)(t, p, &mut noise) * fade).clamp(-1.0, 1.0);
            (v * i16::MAX as f32) as i16
        })
        .collect();
    wav(&samples)
}

#[test]
fn survive3d_sounds_exist_and_are_valid_wavs() {
    let dir = Path::new("examples/survive3d/assets");
    std::fs::create_dir_all(dir).expect("assets dir");
    for r in RECIPES {
        let path = dir.join(r.name);
        if !path.exists() {
            std::fs::write(&path, render(r)).expect("write wav");
        }
        let bytes = std::fs::read(&path).expect("read wav");
        assert_eq!(&bytes[..4], b"RIFF", "{}", r.name);
        assert_eq!(&bytes[8..16], b"WAVEfmt ", "{}", r.name);
        assert!(bytes.len() > 44 + 100, "{} has audio", r.name);
        // Deterministic: regenerating gives the committed bytes.
        assert_eq!(bytes, render(r), "{} differs from its recipe", r.name);
    }
}
