//! Lanternfall G5: the night sky that lights the island, computed.
//!
//! Writes `examples/lanternfall/assets/night.hdr` (a 256 × 128
//! equirectangular Radiance image, flat RGBE scanlines) if it's missing,
//! then checks it, like `gen_survive3d_sky.rs`. `light.environment`
//! lights the island from it: a deep blue night, a soft cool glow around
//! the moon (whose own light is `sun.direction`), a warm haze low on one
//! horizon from the lanterns of the mainland, and faint stars.

use std::path::Path;

const W: usize = 256;
const H: usize = 128;

/// The moon's direction in `main.twe` (`sun.direction`), normalized.
fn moon_dir() -> [f32; 3] {
    let v = [-0.35f32, 0.8, 0.45];
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// A cheap integer hash for the stars.
fn hash(x: usize, y: usize) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (y as u32).wrapping_mul(0x1656_67b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    (h & 0xffff) as f32 / 65535.0
}

/// Linear radiance seen in direction `d` (pixel `x, y` for the stars).
fn sky(d: [f32; 3], x: usize, y: usize) -> [f32; 3] {
    let zenith = [0.012, 0.018, 0.05];
    let horizon = [0.05, 0.06, 0.12];
    let ground = [0.012, 0.014, 0.02];
    let mut c = if d[1] >= 0.0 {
        mix(horizon, zenith, d[1].powf(0.5))
    } else {
        mix(horizon, ground, (-d[1] * 5.0).min(1.0))
    };
    // The moon's halo.
    let m = moon_dir();
    let cos = (d[0] * m[0] + d[1] * m[1] + d[2] * m[2]).max(0.0);
    let halo = cos.powf(64.0) * 1.6 + cos.powf(6.0) * 0.08;
    c = [c[0] + halo * 0.55, c[1] + halo * 0.65, c[2] + halo * 0.9];
    // The mainland's warm haze, low in the south-east.
    let warm = ((d[0] * 0.6 + d[2] * 0.8).max(0.0)).powf(3.0) * (1.0 - d[1].abs()).powf(6.0);
    c = [c[0] + warm * 0.12, c[1] + warm * 0.06, c[2] + warm * 0.02];
    // Stars, above the horizon.
    if d[1] > 0.08 && hash(x, y) > 0.992 {
        let b = 0.5 + hash(y, x) * 1.5;
        c = [c[0] + b * 0.8, c[1] + b * 0.85, c[2] + b];
    }
    c
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
            out.extend(rgbe(sky(d, x, y)));
        }
    }
    out
}

#[test]
fn lanternfall_sky_exists_and_is_an_hdr() {
    let path = Path::new("examples/lanternfall/assets/night.hdr");
    if !path.exists() {
        std::fs::write(path, image()).expect("write night.hdr");
    }
    let bytes = std::fs::read(path).expect("read night.hdr");
    assert!(bytes.starts_with(b"#?RADIANCE"), "Radiance header");
    let header = format!("\n-Y {H} +X {W}\n");
    let at = bytes
        .windows(header.len())
        .position(|w| w == header.as_bytes())
        .expect("resolution line");
    assert_eq!(bytes.len() - (at + header.len()), W * H * 4, "flat RGBE pixels");
    assert_eq!(bytes, image(), "night.hdr is stale: delete it and rerun");
}
