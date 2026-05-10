//! Phase 32 session 5: terrain heightfield with chunk tiling.
//!
//! A terrain is an XZ-aligned grid of height samples. The grid is
//! split into chunks for streaming parity with [`crate::streaming`]
//! — each chunk owns an N×N grid of f32 heights, and chunks are
//! identified by integer (cx, cz) coordinates exactly like the
//! streaming module.
//!
//! ## Chunk shape
//!
//! - `chunk_size` (default 64m): world-space side length of one chunk.
//!   Should usually equal [`crate::streaming::StreamingState::chunk_size`]
//!   so terrain streams in lockstep with prop streaming.
//! - `chunk_resolution` (default 65): samples per side. 65 gives 64
//!   intervals exactly aligned with the chunk_size; the +1 is the
//!   shared edge with the neighboring chunk so adjacent chunks
//!   stitch without a seam.
//!
//! ## What this module ships
//!
//! - [`Terrain::set_chunk`] to install N×N height samples for a given
//!   chunk index.
//! - [`Terrain::height_at`] to bilinearly interpolate the height at
//!   any (x, z) world position.
//! - [`Terrain::normal_at`] to compute a slope normal from the local
//!   height gradient (central-difference, used by gameplay code that
//!   wants a "wet floor / steep slope" classifier).
//!
//! Loading actual heightmap data — from a .png, a procedural noise
//! function, or a baked .ter file — is the script's job. Scripts call
//! `terrain.set_chunk(cx, cz, heights)` with whatever they cooked up.
//!
//! ## What's not in this module
//!
//! - Mesh generation. The wgpu render path generates a triangle
//!   strip from the heightmap; that lives in `play3d.rs` (Phase 32
//!   session 7+ integration).
//! - LOD on terrain meshes (sparser sampling at distance). Tracked
//!   as a follow-on session — extends [`crate::lod`] but needs the
//!   mesh-generation path to land first.
//! - Multi-layer terrain (caves, overhangs). Heightfields are
//!   single-valued per (x, z) by construction. Caves use distinct
//!   prop meshes.
//! - Erosion / sculpting tools. v2 / mod-tooling scope.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct HeightChunk {
    pub heights: Vec<f32>,
}

pub struct Terrain {
    pub chunk_size: f32,
    pub chunk_resolution: u32,
    chunks: HashMap<(i32, i32), HeightChunk>,
}

impl Default for Terrain {
    fn default() -> Self {
        Terrain {
            chunk_size: 64.0,
            chunk_resolution: 65,
            chunks: HashMap::new(),
        }
    }
}

impl Terrain {
    pub fn clear(&mut self) {
        self.chunks.clear();
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Install a chunk's height data. `heights.len()` must equal
    /// `chunk_resolution * chunk_resolution`. Returns an error on
    /// length mismatch so the script gets a clear message instead
    /// of silent corruption.
    pub fn set_chunk(&mut self, cx: i32, cz: i32, heights: Vec<f32>) -> Result<(), String> {
        let expected = (self.chunk_resolution as usize).pow(2);
        if heights.len() != expected {
            return Err(format!(
                "terrain.set_chunk: heights has {} samples, expected {} ({}×{})",
                heights.len(),
                expected,
                self.chunk_resolution,
                self.chunk_resolution
            ));
        }
        self.chunks.insert((cx, cz), HeightChunk { heights });
        Ok(())
    }

    /// True if a given chunk has been registered.
    pub fn has_chunk(&self, cx: i32, cz: i32) -> bool {
        self.chunks.contains_key(&(cx, cz))
    }

    /// World position → chunk index.
    pub fn chunk_of(&self, x: f32, z: f32) -> (i32, i32) {
        let inv = 1.0 / self.chunk_size;
        ((x * inv).floor() as i32, (z * inv).floor() as i32)
    }

    /// Bilinearly interpolate the height at world position `(x, z)`.
    /// Returns `None` if the chunk covering `(x, z)` isn't loaded —
    /// scripts decide what to do (skip the spawn, queue a stream
    /// request, fall back to a sentinel). Returning `None` rather
    /// than a default is Principle-3 friendly: a zero height could
    /// be silently wrong; an explicit None forces the caller to
    /// handle the gap.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let (cx, cz) = self.chunk_of(x, z);
        let chunk = self.chunks.get(&(cx, cz))?;
        let res = self.chunk_resolution as f32;

        // Local coordinates within the chunk, in sample units.
        let local_x = (x - (cx as f32) * self.chunk_size) / self.chunk_size * (res - 1.0);
        let local_z = (z - (cz as f32) * self.chunk_size) / self.chunk_size * (res - 1.0);

        let i0 = local_x.floor() as i32;
        let i1 = (i0 + 1).min(self.chunk_resolution as i32 - 1);
        let j0 = local_z.floor() as i32;
        let j1 = (j0 + 1).min(self.chunk_resolution as i32 - 1);

        // Clamp into [0, res-1] in case of f32 precision pushing
        // beyond the boundary.
        let i0 = i0.clamp(0, self.chunk_resolution as i32 - 1);
        let j0 = j0.clamp(0, self.chunk_resolution as i32 - 1);

        let tx = (local_x - i0 as f32).clamp(0.0, 1.0);
        let tz = (local_z - j0 as f32).clamp(0.0, 1.0);

        let res_u = self.chunk_resolution as usize;
        let h00 = chunk.heights[(j0 as usize) * res_u + i0 as usize];
        let h10 = chunk.heights[(j0 as usize) * res_u + i1 as usize];
        let h01 = chunk.heights[(j1 as usize) * res_u + i0 as usize];
        let h11 = chunk.heights[(j1 as usize) * res_u + i1 as usize];

        let h0 = h00 + (h10 - h00) * tx;
        let h1 = h01 + (h11 - h01) * tx;
        Some(h0 + (h1 - h0) * tz)
    }

    /// Approximate surface normal at `(x, z)` via central-difference
    /// over a small `eps` step. Returns a unit-length 3-vector
    /// (Y is up). Returns `None` if any of the four sample points
    /// falls in an unloaded chunk.
    pub fn normal_at(&self, x: f32, z: f32) -> Option<[f32; 3]> {
        let eps = self.chunk_size / (self.chunk_resolution as f32 - 1.0);
        let h_xp = self.height_at(x + eps, z)?;
        let h_xn = self.height_at(x - eps, z)?;
        let h_zp = self.height_at(x, z + eps)?;
        let h_zn = self.height_at(x, z - eps)?;
        // Tangents along x and z. Cross product (tx × tz) gives the
        // normal pointing "up" given the right-handed XZ-up
        // convention used elsewhere in the engine.
        let dx = (h_xp - h_xn) / (2.0 * eps);
        let dz = (h_zp - h_zn) / (2.0 * eps);
        // The unnormalized normal is (-dx, 1, -dz).
        let n = [-dx, 1.0, -dz];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        Some([n[0] / len, n[1] / len, n[2] / len])
    }
}

pub static TERRAIN: Mutex<Option<Terrain>> = Mutex::new(None);

pub fn with_terrain<R>(f: impl FnOnce(&mut Terrain) -> R) -> R {
    let mut guard = TERRAIN.lock().expect("terrain mutex poisoned");
    if guard.is_none() {
        *guard = Some(Terrain::default());
    }
    f(guard.as_mut().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_chunk(t: &Terrain, h: f32) -> Vec<f32> {
        vec![h; (t.chunk_resolution as usize).pow(2)]
    }

    fn ramp_chunk(t: &Terrain, slope: f32) -> Vec<f32> {
        // height = i * slope (i.e. ramps along +x).
        let r = t.chunk_resolution as usize;
        let mut out = Vec::with_capacity(r * r);
        for _ in 0..r {
            for i in 0..r {
                out.push(i as f32 * slope);
            }
        }
        out
    }

    #[test]
    fn set_chunk_rejects_wrong_length() {
        let mut t = Terrain::default();
        assert!(t.set_chunk(0, 0, vec![0.0; 100]).is_err());
        assert!(t.set_chunk(0, 0, flat_chunk(&t, 0.0)).is_ok());
    }

    #[test]
    fn height_at_returns_none_for_unloaded_chunks() {
        let t = Terrain::default();
        assert_eq!(t.height_at(0.0, 0.0), None);
        assert_eq!(t.height_at(1000.0, 1000.0), None);
    }

    #[test]
    fn flat_chunk_reads_back_constant_height() {
        let mut t = Terrain::default();
        let heights = flat_chunk(&t, 12.5);
        t.set_chunk(0, 0, heights).unwrap();
        // Sample at several points within chunk (0,0).
        for &(x, z) in &[(0.0, 0.0), (10.0, 30.0), (60.0, 60.0), (32.0, 32.0)] {
            let h = t.height_at(x, z).expect("loaded");
            assert!(
                (h - 12.5).abs() < 1e-4,
                "expected 12.5, got {h} at ({x}, {z})"
            );
        }
    }

    #[test]
    fn ramp_chunk_interpolates_linearly() {
        let mut t = Terrain::default();
        let heights = ramp_chunk(&t, 1.0);
        t.set_chunk(0, 0, heights).unwrap();
        // chunk_size=64, chunk_resolution=65 → 64 intervals over 64m,
        // so 1m world = 1 sample = 1.0 height units.
        let h0 = t.height_at(0.0, 0.0).unwrap();
        let h32 = t.height_at(32.0, 0.0).unwrap();
        // Half the chunk → height ≈ 32.
        assert!((h32 - 32.0).abs() < 1e-3, "got {h32}");
        assert!(h0.abs() < 1e-3);
    }

    #[test]
    fn ramp_chunk_normal_points_correctly() {
        let mut t = Terrain::default();
        let heights = ramp_chunk(&t, 1.0);
        t.set_chunk(0, 0, heights).unwrap();
        // For a linear ramp h = x (height grows 1 unit per 1m of x),
        // the normal should be in the (-1, 1, 0) plane (downhill toward
        // -x), normalized. Y component ≈ 1/sqrt(2) ≈ 0.7071.
        let n = t.normal_at(20.0, 20.0).expect("loaded");
        assert!(
            (n[1] - 1.0 / 2f32.sqrt()).abs() < 1e-3,
            "expected y≈0.7071, got {}",
            n[1]
        );
        assert!(n[0] < 0.0, "x-component should be negative for +x ramp");
        assert!(n[2].abs() < 1e-3, "z-component should be ~0 for x-only ramp");
    }

    #[test]
    fn flat_chunk_normal_points_straight_up() {
        let mut t = Terrain::default();
        let heights = flat_chunk(&t, 5.0);
        t.set_chunk(0, 0, heights).unwrap();
        let n = t.normal_at(32.0, 32.0).expect("loaded");
        assert!((n[0]).abs() < 1e-4);
        assert!((n[1] - 1.0).abs() < 1e-4);
        assert!((n[2]).abs() < 1e-4);
    }

    #[test]
    fn with_terrain_initializes_lazily() {
        with_terrain(|t| {
            t.clear();
            t.set_chunk(0, 0, flat_chunk(t, 7.0)).unwrap();
            assert_eq!(t.chunk_count(), 1);
            assert!((t.height_at(10.0, 10.0).unwrap() - 7.0).abs() < 1e-4);
            t.clear();
        });
    }
}
