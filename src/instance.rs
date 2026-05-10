//! Phase 32 session 7: per-asset instance buckets — extension of
//! the Phase 23 dynamic instance buffer.
//!
//! Phase 23 shipped a dynamic instance buffer that grows past the
//! original 4096-instance cap, but it assumed a single asset per
//! draw call. Open-world scenes have hundreds of asset variants
//! (trees, rocks, NPCs, props at multiple LODs) — submitting one
//! instanced draw per (asset, LOD) is the standard renderer shape.
//!
//! This module ships the *bookkeeping* for that shape: a
//! `HashMap<asset_path, Vec<Transform>>` rebuilt every frame from
//! the visible-set query (Phase 32 session 6 frustum-cull → Phase 32
//! session 4 LOD-select → bucket here). The wgpu render integration
//! that consumes the buckets — uploading them to a single growable
//! storage buffer with per-bucket (offset, length) pairs and
//! issuing one `draw_indexed_indirect` per bucket — lands in a
//! follow-on render-integration session.
//!
//! ## Why script-side bucketing
//!
//! Bucketing in Twe rather than Rust keeps the renderer-data flow
//! observable from gameplay code. A script that wants to add a
//! frame-spike-debugging "show buckets" overlay queries
//! [`bucket_count`] / [`total_instances`] directly. A future
//! `world.instance_add` integration with `entity` blocks will
//! call into this module from the engine side, but the primitives
//! stay script-visible.
//!
//! ## Layout
//!
//! Each instance is a 4×4 transform stored as a flat `[f32; 16]`
//! row-major. That matches the wgpu storage-buffer layout (16-byte
//! aligned, four `vec4<f32>` per instance) and the Phase 23
//! existing transform format. Scripts produce transforms via
//! `mat4.*` (Phase 19) or via per-component translation/rotation/
//! scale mul-chains.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct InstanceBuckets {
    buckets: HashMap<String, Vec<[f32; 16]>>,
}

impl InstanceBuckets {
    pub fn clear(&mut self) {
        for v in self.buckets.values_mut() {
            v.clear();
        }
    }

    /// Wipe everything including the asset keys themselves. Use
    /// when the LOD chains change mid-run; otherwise [`clear`] is
    /// the right per-frame reset (preserves the per-asset Vec
    /// allocations).
    pub fn reset(&mut self) {
        self.buckets.clear();
    }

    pub fn add(&mut self, asset: &str, transform: [f32; 16]) {
        self.buckets
            .entry(asset.to_string())
            .or_default()
            .push(transform);
    }

    pub fn count(&self, asset: &str) -> usize {
        self.buckets.get(asset).map(|v| v.len()).unwrap_or(0)
    }

    pub fn assets(&self) -> Vec<String> {
        let mut v: Vec<String> = self.buckets.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn transforms(&self, asset: &str) -> Vec<[f32; 16]> {
        self.buckets
            .get(asset)
            .cloned()
            .unwrap_or_default()
    }

    /// Sum of instance counts across all buckets. Useful for the
    /// "we're rendering N props" HUD line and for asserting the
    /// frustum cull actually shrank the visible set.
    pub fn total_instances(&self) -> usize {
        self.buckets.values().map(|v| v.len()).sum()
    }

    pub fn bucket_count(&self) -> usize {
        self.buckets.iter().filter(|(_, v)| !v.is_empty()).count()
    }
}

pub static BUCKETS: Mutex<Option<InstanceBuckets>> = Mutex::new(None);

pub fn with_buckets<R>(f: impl FnOnce(&mut InstanceBuckets) -> R) -> R {
    let mut guard = BUCKETS.lock().expect("instance buckets mutex poisoned");
    if guard.is_none() {
        *guard = Some(InstanceBuckets::default());
    }
    f(guard.as_mut().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_transform() -> [f32; 16] {
        [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]
    }

    fn translate(x: f32, y: f32, z: f32) -> [f32; 16] {
        [
            1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, y, 0.0, 0.0, 1.0, z, 0.0, 0.0, 0.0, 1.0,
        ]
    }

    #[test]
    fn add_groups_by_asset() {
        let mut b = InstanceBuckets::default();
        b.add("tree.glb", id_transform());
        b.add("tree.glb", translate(10.0, 0.0, 0.0));
        b.add("rock.glb", id_transform());
        assert_eq!(b.count("tree.glb"), 2);
        assert_eq!(b.count("rock.glb"), 1);
        assert_eq!(b.count("missing.glb"), 0);
    }

    #[test]
    fn assets_returns_sorted_unique_keys() {
        let mut b = InstanceBuckets::default();
        b.add("c.glb", id_transform());
        b.add("a.glb", id_transform());
        b.add("b.glb", id_transform());
        b.add("a.glb", id_transform());
        assert_eq!(b.assets(), vec!["a.glb", "b.glb", "c.glb"]);
    }

    #[test]
    fn clear_drops_instances_but_keeps_keys() {
        let mut b = InstanceBuckets::default();
        b.add("tree.glb", id_transform());
        b.clear();
        assert_eq!(b.count("tree.glb"), 0);
        // Bucket count counts only non-empty buckets.
        assert_eq!(b.bucket_count(), 0);
        // The asset list still contains "tree.glb" so the next-frame
        // re-add doesn't have to grow the HashMap.
        assert_eq!(b.assets(), vec!["tree.glb".to_string()]);
    }

    #[test]
    fn reset_drops_keys_too() {
        let mut b = InstanceBuckets::default();
        b.add("tree.glb", id_transform());
        b.reset();
        assert!(b.assets().is_empty());
    }

    #[test]
    fn total_instances_sums_all_buckets() {
        let mut b = InstanceBuckets::default();
        for i in 0..5 {
            b.add("tree.glb", translate(i as f32, 0.0, 0.0));
        }
        for i in 0..3 {
            b.add("rock.glb", translate(0.0, 0.0, i as f32));
        }
        assert_eq!(b.total_instances(), 8);
        assert_eq!(b.bucket_count(), 2);
    }

    #[test]
    fn transforms_returns_in_insertion_order() {
        let mut b = InstanceBuckets::default();
        b.add("tree.glb", translate(1.0, 0.0, 0.0));
        b.add("tree.glb", translate(2.0, 0.0, 0.0));
        b.add("tree.glb", translate(3.0, 0.0, 0.0));
        let xs: Vec<f32> = b.transforms("tree.glb").iter().map(|t| t[3]).collect();
        assert_eq!(xs, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn with_buckets_initializes_lazily() {
        with_buckets(|b| {
            b.reset();
            b.add("test.glb", id_transform());
            assert_eq!(b.count("test.glb"), 1);
            b.reset();
        });
    }
}
