//! web3d-M7 session 16: the sky that lights `examples/survive3d`,
//! computed.
//!
//! Writes `examples/survive3d/assets/sky.hdr` (a 256 × 128
//! equirectangular Radiance image, flat RGBE scanlines) if it's
//! missing, then checks it. The file is committed, so players never run
//! this; it is the procedural spec of the sky (edit the gradient, delete
//! the file, rerun to regenerate). `light.environment` lights the arena
//! from it: a dusk sky, cool overhead and warm at the horizon, with a
//! soft glow toward the sun (the sun's own light comes from
//! `sun.direction`, so the sky carries no sharp disk to count twice).

use std::path::Path;

const W: usize = 256;
const H: usize = 128;

/// The sun's direction in `survive3d` (`sun.direction`), normalized.
fn sun_dir() -> [f32; 3] {
    let v = [-0.4f32, 0.8, 0.35];
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Linear radiance seen in direction `d`.
fn sky(d: [f32; 3]) -> [f32; 3] {
    let zenith = [0.10, 0.16, 0.38];
    let horizon = [0.95, 0.52, 0.30];
    let ground = [0.07, 0.055, 0.05];
    let base = if d[1] >= 0.0 {
        // Warm near the horizon, cooling quickly with height.
        mix(horizon, zenith, d[1].powf(0.45))
    } else {
        mix(horizon, ground, (-d[1] * 6.0).min(1.0))
    };
    let s = sun_dir();
    let cos = (d[0] * s[0] + d[1] * s[1] + d[2] * s[2]).max(0.0);
    let glow = cos.powf(24.0) * 6.0 + cos.powf(4.0) * 0.4;
    [base[0] + glow * 1.0, base[1] + glow * 0.75, base[2] + glow * 0.5]
}

fn rgbe(c: [f32; 3]) -> [u8; 4] {
    let max = c[0].max(c[1]).max(c[2]);
    if max < 1e-32 {
        return [0, 0, 0, 0];
    }
    let e = max.log2().floor() as i32 + 1;
    let scale = 256.0 / 2f32.powi(e);
    [
        (c[0] * scale) as u8,
        (c[1] * scale) as u8,
        (c[2] * scale) as u8,
        (e + 128) as u8,
    ]
}

fn image() -> Vec<u8> {
    let mut out = format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {H} +X {W}\n").into_bytes();
    let pi = std::f32::consts::PI;
    for y in 0..H {
        // The kernel's mapping: v = 0.5 - asin(d.y) / π,
        // u = atan2(d.z, d.x) / 2π + 0.5.
        let el = (0.5 - (y as f32 + 0.5) / H as f32) * pi;
        for x in 0..W {
            let az = ((x as f32 + 0.5) / W as f32 - 0.5) * 2.0 * pi;
            let d = [el.cos() * az.cos(), el.sin(), el.cos() * az.sin()];
            out.extend(rgbe(sky(d)));
        }
    }
    out
}

#[test]
fn survive3d_sky_exists_and_is_an_hdr() {
    let path = Path::new("examples/survive3d/assets/sky.hdr");
    if !path.exists() {
        std::fs::write(path, image()).expect("write sky.hdr");
    }
    let bytes = std::fs::read(path).expect("read sky.hdr");
    assert!(bytes.starts_with(b"#?RADIANCE"), "Radiance header");
    let header = format!("\n-Y {H} +X {W}\n");
    let at = bytes
        .windows(header.len())
        .position(|w| w == header.as_bytes())
        .expect("resolution line");
    assert_eq!(bytes.len() - (at + header.len()), W * H * 4, "flat RGBE pixels");
    // The committed file is what the recipe makes.
    assert_eq!(bytes, image(), "sky.hdr is stale: delete it and rerun");
}
