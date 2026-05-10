//! Phase 32 session 3: chunked scene streaming.
//!
//! Splits a large open world into fixed-size XZ chunks (default
//! 64m × 64m). Each frame the streaming state machine compares the
//! camera position against `stream_radius` and the current loaded
//! set:
//!
//! - **To load:** chunks within `stream_radius` that aren't loaded.
//! - **To unload:** chunks beyond `stream_radius + hysteresis` that
//!   are currently loaded. The hysteresis band prevents thrashing
//!   when the camera hovers on a chunk boundary.
//!
//! This module is the *bookkeeping* — what should be loaded, what
//! should be unloaded, and a per-frame budget so a single chunk
//! stall doesn't drop a frame. The actual asset I/O (mesh load,
//! texture decode, entity spawn) lives in script callbacks driven
//! by the [`step`] method's `to_load` / `to_unload` outputs. Decoupling
//! "what to load" from "how to load it" matches the engine-internal
//! worker pool model authorized by the Phase 32 session 1 lock
//! revision: bookkeeping stays on the main thread, asset I/O
//! eventually moves to workers.
//!
//! ## Per-frame budget
//!
//! The exit criterion calls for "budget-bound async loads" — at
//! 60Hz with a 16ms frame budget, a single chunk's mesh load can
//! easily eat 5–20ms. To keep the frame stable, [`step`] returns
//! at most `loads_per_frame` and `unloads_per_frame` chunk ids.
//! Pending work spills to the next frame. The budget defaults are
//! conservative (2 loads, 2 unloads per frame); a fast SSD + mesh
//! cache can crank these higher.
//!
//! ## What's not in this module
//!
//! - Asset cache. The script keeps its own — typically a HashMap
//!   keyed on chunk id. The streaming machinery doesn't dictate
//!   storage shape.
//! - Physics. Per-chunk rapier colliders are added when the script
//!   marks the chunk loaded; same teardown story for unload.
//! - LOD selection. Chunks are loaded at full detail; per-mesh LOD
//!   selection is Phase 32 session 4.
//! - Async-IO worker dispatch. This module returns "what to load";
//!   the worker dispatch is an integration that lands in a
//!   follow-on session (session 4 is mesh+texture LOD, session 5
//!   is terrain heightfield, etc.).

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashSet;

/// Chunk identifier — packed `(x, z)` integer cell coordinates.
/// Encoded as `(x << 32) | (z & 0xffffffff)` so a single u64 round
/// trip keeps the API small. Chunks at extreme negative coordinates
/// (≤ i32::MIN) saturate; the world is effectively i32-bounded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ChunkId(pub u64);

impl ChunkId {
    pub fn new(x: i32, z: i32) -> Self {
        ChunkId(((x as u64 & 0xffff_ffff) << 32) | (z as u64 & 0xffff_ffff))
    }

    pub fn xz(self) -> (i32, i32) {
        let x = (self.0 >> 32) as i32;
        let z = (self.0 & 0xffff_ffff) as i32;
        (x, z)
    }
}

/// Streaming state machine. One instance per process — exposed via
/// the global [`STREAMING`] mutex.
pub struct StreamingState {
    /// World-space chunk size in meters. Default 64.0 fits Tunic
    /// scale (a 4km world = 62 chunks × 62 chunks × 64m each).
    pub chunk_size: f32,
    /// Stream-in radius in chunk units (1 chunk = chunk_size meters).
    /// Default 4 chunks ≈ 256m vision distance.
    pub stream_radius_chunks: i32,
    /// Hysteresis band beyond stream_radius_chunks before unloading.
    /// Prevents the load/unload thrash when the camera dwells on a
    /// chunk boundary.
    pub unload_hysteresis_chunks: i32,
    /// Per-frame load / unload caps. Pending work spills to the next
    /// frame; the player just sees chunks pop in over a few frames
    /// after fast travel rather than a single dropped frame.
    pub loads_per_frame: u32,
    pub unloads_per_frame: u32,
    /// Currently loaded chunk ids. The script confirms loads via
    /// [`mark_loaded`] / [`mark_unloaded`] — until then they're
    /// in `loading` / `unloading` and don't gate the next round.
    loaded: HashSet<ChunkId>,
    loading: HashSet<ChunkId>,
    unloading: HashSet<ChunkId>,
    /// Last camera chunk seen by [`step`]. Diffing against this
    /// short-circuits the per-frame computation when the camera
    /// hasn't crossed a chunk boundary.
    last_camera_chunk: Option<(i32, i32)>,
}

impl Default for StreamingState {
    fn default() -> Self {
        StreamingState {
            chunk_size: 64.0,
            stream_radius_chunks: 4,
            unload_hysteresis_chunks: 1,
            loads_per_frame: 2,
            unloads_per_frame: 2,
            loaded: HashSet::new(),
            loading: HashSet::new(),
            unloading: HashSet::new(),
            last_camera_chunk: None,
        }
    }
}

/// Per-frame streaming work.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct StreamingStep {
    pub to_load: Vec<ChunkId>,
    pub to_unload: Vec<ChunkId>,
}

impl StreamingState {
    pub fn clear(&mut self) {
        self.loaded.clear();
        self.loading.clear();
        self.unloading.clear();
        self.last_camera_chunk = None;
    }

    pub fn loaded_chunks(&self) -> Vec<ChunkId> {
        let mut v: Vec<ChunkId> = self.loaded.iter().copied().collect();
        v.sort_by_key(|c| c.0);
        v
    }

    /// Number of chunks currently loaded (excludes in-flight).
    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    pub fn loading_count(&self) -> usize {
        self.loading.len()
    }

    pub fn unloading_count(&self) -> usize {
        self.unloading.len()
    }

    /// Camera position → chunk coordinates (XZ).
    pub fn camera_chunk(&self, x: f32, z: f32) -> (i32, i32) {
        let inv = 1.0 / self.chunk_size;
        let cx = (x * inv).floor() as i32;
        let cz = (z * inv).floor() as i32;
        (cx, cz)
    }

    /// Mark a chunk as fully loaded by the script — moves it from
    /// `loading` to `loaded`. No-op if it wasn't in flight.
    pub fn mark_loaded(&mut self, c: ChunkId) {
        if self.loading.remove(&c) {
            self.loaded.insert(c);
        } else {
            // Tolerated: scripts that build their world up front
            // (no loading flow) can call mark_loaded directly.
            self.loaded.insert(c);
        }
    }

    /// Confirm an unload completed — moves chunk out of `unloading`.
    /// If it was never being unloaded, this is a no-op.
    pub fn mark_unloaded(&mut self, c: ChunkId) {
        self.unloading.remove(&c);
        self.loaded.remove(&c);
    }

    /// Compute one frame's worth of streaming work given the camera
    /// position. Returns up to `loads_per_frame` chunk ids that
    /// should be loaded and up to `unloads_per_frame` chunk ids
    /// that should be unloaded. The state machine internally moves
    /// them to `loading` / `unloading`; the script then dispatches
    /// asset I/O and confirms back via `mark_loaded` / `mark_unloaded`.
    pub fn step(&mut self, camera_x: f32, camera_z: f32) -> StreamingStep {
        let cam = self.camera_chunk(camera_x, camera_z);
        self.last_camera_chunk = Some(cam);

        // Compute the in-radius set.
        let r = self.stream_radius_chunks;
        let mut in_radius: HashSet<ChunkId> = HashSet::new();
        for dx in -r..=r {
            for dz in -r..=r {
                if dx * dx + dz * dz <= r * r {
                    in_radius.insert(ChunkId::new(cam.0 + dx, cam.1 + dz));
                }
            }
        }

        // Find loads: in_radius minus (loaded ∪ loading), capped.
        let mut to_load: Vec<ChunkId> = in_radius
            .iter()
            .filter(|c| !self.loaded.contains(c) && !self.loading.contains(c))
            .copied()
            .collect();
        // Sort by Manhattan distance from camera so the closest
        // chunks load first — the player sees the immediate
        // surroundings before the periphery.
        to_load.sort_by_key(|c| {
            let (x, z) = c.xz();
            (x - cam.0).abs() + (z - cam.1).abs()
        });
        to_load.truncate(self.loads_per_frame as usize);

        // Find unloads: loaded chunks beyond
        // stream_radius + hysteresis.
        let cutoff = self.stream_radius_chunks + self.unload_hysteresis_chunks;
        let cutoff_sq = cutoff * cutoff;
        let mut to_unload: Vec<ChunkId> = self
            .loaded
            .iter()
            .filter(|c| {
                let (x, z) = c.xz();
                let dx = x - cam.0;
                let dz = z - cam.1;
                dx * dx + dz * dz > cutoff_sq
            })
            .copied()
            .collect();
        // Unload furthest first so memory reclaims happen on the
        // truly-out-of-range chunks before the marginal ones.
        to_unload.sort_by_key(|c| {
            let (x, z) = c.xz();
            -((x - cam.0).abs() + (z - cam.1).abs())
        });
        to_unload.truncate(self.unloads_per_frame as usize);

        // Move them into the in-flight sets so the next step()
        // doesn't double-count.
        for c in &to_load {
            self.loading.insert(*c);
        }
        for c in &to_unload {
            self.unloading.insert(*c);
            self.loaded.remove(c);
        }
        StreamingStep {
            to_load,
            to_unload,
        }
    }
}

use std::sync::Mutex;

/// Process-wide streaming state. Mutex-protected for parity with
/// `crate::spatial::WORLD` — both are touched from script builtins
/// and (eventually) from a worker pool.
pub static STREAMING: Mutex<Option<StreamingState>> = Mutex::new(None);

pub fn with_streaming<R>(f: impl FnOnce(&mut StreamingState) -> R) -> R {
    let mut guard = STREAMING.lock().expect("streaming mutex poisoned");
    if guard.is_none() {
        *guard = Some(StreamingState::default());
    }
    f(guard.as_mut().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_id_round_trips() {
        for &(x, z) in &[
            (0, 0),
            (1, 1),
            (-1, -1),
            (i32::MAX, i32::MIN),
            (i32::MIN, i32::MAX),
            (12345, -67890),
        ] {
            let c = ChunkId::new(x, z);
            assert_eq!(c.xz(), (x, z));
        }
    }

    #[test]
    fn step_loads_chunks_within_radius() {
        let mut s = StreamingState {
            stream_radius_chunks: 2,
            loads_per_frame: 100,
            unloads_per_frame: 100,
            ..Default::default()
        };
        let step = s.step(0.0, 0.0);
        // r=2, disk of radius 2 (Euclidean), unique chunks where
        // dx^2+dz^2 <= 4 → 13 chunks.
        assert_eq!(step.to_load.len(), 13);
        assert_eq!(step.to_unload.len(), 0);
    }

    #[test]
    fn step_respects_loads_per_frame_budget() {
        let mut s = StreamingState {
            stream_radius_chunks: 4,
            loads_per_frame: 3,
            unloads_per_frame: 3,
            ..Default::default()
        };
        let step = s.step(0.0, 0.0);
        assert_eq!(step.to_load.len(), 3);
        // Closest 3 chunks first: the camera-cell + the four adjacent
        // chunks all have Manhattan distance ≤ 1; the cap of 3 picks
        // a mix of those (sort is stable on ties from HashSet order
        // but the bucket is well-defined).
        for c in &step.to_load {
            let (x, z) = c.xz();
            assert!(
                x.abs() + z.abs() <= 1,
                "expected closest chunks first; got ({x}, {z})"
            );
        }
    }

    #[test]
    fn marking_loaded_then_far_camera_triggers_unload() {
        let mut s = StreamingState {
            stream_radius_chunks: 2,
            unload_hysteresis_chunks: 1,
            loads_per_frame: 100,
            unloads_per_frame: 100,
            ..Default::default()
        };
        let step = s.step(0.0, 0.0);
        for c in &step.to_load {
            s.mark_loaded(*c);
        }
        assert_eq!(s.loaded_count(), 13);

        // Move the camera 1000m away (far outside cutoff = 3 chunks
        // × 64m = 192m). Every previously loaded chunk should
        // unload.
        let step = s.step(1000.0, 0.0);
        assert_eq!(step.to_unload.len(), 13);
        // Confirm them, count goes to 0.
        for c in &step.to_unload {
            s.mark_unloaded(*c);
        }
        assert_eq!(s.loaded_count(), 0);
    }

    #[test]
    fn hysteresis_keeps_marginal_chunks_loaded() {
        let mut s = StreamingState {
            stream_radius_chunks: 2,
            unload_hysteresis_chunks: 1,
            loads_per_frame: 100,
            unloads_per_frame: 100,
            ..Default::default()
        };
        // Load everything within radius 2 from origin.
        let step = s.step(0.0, 0.0);
        for c in &step.to_load {
            s.mark_loaded(*c);
        }
        // Move camera ONE chunk in +x. Radius is 2; chunks that are
        // now at distance up to 3 from camera (radius + hysteresis)
        // should NOT unload yet. The chunk at (-2, 0) is now at
        // distance 3 from camera (1, 0) — exactly on the cutoff,
        // stays loaded. (cutoff_sq = 9; dx^2+dz^2 = 9 is not > 9.)
        let step = s.step(s.chunk_size, 0.0);
        // No unloads — every previously loaded chunk is still
        // within hysteresis.
        assert_eq!(step.to_unload.len(), 0);
    }

    #[test]
    fn step_does_not_reload_chunks_already_in_flight() {
        let mut s = StreamingState {
            stream_radius_chunks: 1,
            loads_per_frame: 100,
            unloads_per_frame: 100,
            ..Default::default()
        };
        let step = s.step(0.0, 0.0);
        let initial_loads = step.to_load.len();
        assert!(initial_loads > 0);

        // Step again WITHOUT mark_loaded — the chunks are now in
        // `loading`. step() should not re-emit them.
        let step = s.step(0.0, 0.0);
        assert_eq!(step.to_load.len(), 0);
    }

    #[test]
    fn with_streaming_initializes_lazily() {
        with_streaming(|s| {
            s.clear();
            let step = s.step(0.0, 0.0);
            assert!(!step.to_load.is_empty());
            s.clear();
        });
    }
}
