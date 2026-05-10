//! Phase 32 session 4: mesh + texture level-of-detail chains.
//!
//! A [`LodChain`] is a per-class declaration of "use this asset at
//! this distance from the camera." The script-facing API:
//!
//! ```text
//! world.set_lod_chain(
//!     "Tree",
//!     ["tree_high.glb", "tree_med.glb", "tree_far.glb", "tree_imposter.glb"],
//!     [25.0, 80.0, 200.0],   # switch distances (in meters)
//! )
//! let asset = world.lod_for_distance("Tree", distance_to_camera)
//! ```
//!
//! Switch distances are between adjacent assets:
//! - distance < 25m   → asset[0] (tree_high.glb)
//! - 25 ≤ distance < 80   → asset[1] (tree_med.glb)
//! - 80 ≤ distance < 200  → asset[2] (tree_far.glb)
//! - 200 ≤ distance       → asset[3] (tree_imposter.glb)
//!
//! Scripts call `world.lod_for_distance` per render frame per
//! visible entity (frustum-culled to a small list, see Phase 32
//! session 6) to pick the right asset. The renderer then issues a
//! single instanced draw per (asset, transform-list) pair.
//!
//! ## Texture LOD vs mesh LOD
//!
//! Texture LOD (mipmaps) is shipped — Phase 28 session 1 adds the
//! mipmap pyramid + 16× anisotropic filtering pipeline. The LOD
//! API in *this* module is mesh-level: swap a 5000-tri tree for a
//! 200-tri tree at distance, then a 2-tri imposter quad farther
//! out. Script-side texture LOD ("use this PNG up close, this one
//! far") maps onto mesh LOD by separating into different .glb
//! files with different texture references — no separate texture
//! API is needed.
//!
//! ## What's not in this module
//!
//! - GPU instancing buffer expansion. Phase 32 session 7 picks up
//!   the per-LOD instance bucket; for now scripts can hand-bucket
//!   into N draw calls.
//! - Smooth LOD transition (alpha-blend two LODs across a 5m band).
//!   Visual polish, not gating any v1.0 use case. Tracked as a
//!   follow-on session.
//! - Hysteresis on LOD switch (so an entity hovering at the 25m
//!   boundary doesn't pop). Easy follow-on; default behavior is
//!   strict-distance for predictability.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::sync::Mutex;

/// A level-of-detail chain for one entity class. `assets[i]` is
/// active when `switch_distances[i-1] ≤ distance < switch_distances[i]`
/// (with implicit −∞ on one end and +∞ on the other).
#[derive(Clone, Debug, PartialEq)]
pub struct LodChain {
    pub assets: Vec<String>,
    pub switch_distances: Vec<f32>,
}

impl LodChain {
    /// Construct + validate a LOD chain. Returns an error string if
    /// the inputs disagree on length, the assets are empty, or the
    /// switch distances aren't strictly increasing.
    pub fn new(assets: Vec<String>, switch_distances: Vec<f32>) -> Result<Self, String> {
        if assets.is_empty() {
            return Err("LOD chain must have at least one asset".to_string());
        }
        if switch_distances.len() + 1 != assets.len() {
            return Err(format!(
                "LOD chain has {} assets but {} switch distances; expected {} switches",
                assets.len(),
                switch_distances.len(),
                assets.len() - 1
            ));
        }
        for w in switch_distances.windows(2) {
            if w[0] >= w[1] {
                return Err(format!(
                    "LOD switch distances must be strictly increasing (got {} >= {})",
                    w[0], w[1]
                ));
            }
        }
        for d in &switch_distances {
            if *d < 0.0 {
                return Err(format!("LOD switch distance must be non-negative (got {d})"));
            }
        }
        Ok(LodChain {
            assets,
            switch_distances,
        })
    }

    /// Pick the asset index for a camera-to-entity distance. Always
    /// returns a valid index into `self.assets`.
    pub fn select(&self, distance: f32) -> usize {
        // Binary search for clarity and asymptotic correctness; with
        // 4-element typical chains a linear scan is the same speed,
        // but the search reads as the intent ("find first switch
        // greater than distance, that's the index").
        match self
            .switch_distances
            .binary_search_by(|d| d.partial_cmp(&distance).unwrap_or(std::cmp::Ordering::Equal))
        {
            // Distance equals a switch boundary → use the higher-index
            // (less-detailed) LOD. The convention picks "≤" for the
            // less-detailed side so an entity sitting exactly at 25m
            // uses tree_med.glb, not tree_high.glb. Predictable.
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }

    /// Convenience: return the asset path for a distance directly.
    pub fn asset_for_distance(&self, distance: f32) -> &str {
        &self.assets[self.select(distance)]
    }
}

/// Process-wide LOD table — class name → chain. Scripts register
/// chains once at scene init; queries are read-only afterwards.
pub static LOD_TABLE: Mutex<Option<HashMap<String, LodChain>>> = Mutex::new(None);

pub fn with_table<R>(f: impl FnOnce(&mut HashMap<String, LodChain>) -> R) -> R {
    let mut guard = LOD_TABLE.lock().expect("lod table mutex poisoned");
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    f(guard.as_mut().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(assets: &[&str], switches: &[f32]) -> LodChain {
        LodChain::new(
            assets.iter().map(|s| s.to_string()).collect(),
            switches.to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn rejects_mismatched_lengths() {
        assert!(LodChain::new(vec!["a.glb".to_string()], vec![10.0]).is_err());
        assert!(LodChain::new(
            vec!["a.glb".to_string(), "b.glb".to_string()],
            vec![10.0, 20.0]
        )
        .is_err());
        assert!(LodChain::new(Vec::new(), Vec::new()).is_err());
    }

    #[test]
    fn rejects_non_increasing_switches() {
        assert!(LodChain::new(
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec![20.0, 10.0],
        )
        .is_err());
        // Equal distances are also rejected — there's no
        // unambiguous LOD at the duplicate point.
        assert!(LodChain::new(
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec![10.0, 10.0],
        )
        .is_err());
    }

    #[test]
    fn rejects_negative_switches() {
        assert!(LodChain::new(
            vec!["a".to_string(), "b".to_string()],
            vec![-1.0],
        )
        .is_err());
    }

    #[test]
    fn select_picks_correct_index() {
        let c = chain(&["high", "med", "far", "imp"], &[25.0, 80.0, 200.0]);
        assert_eq!(c.select(0.0), 0);
        assert_eq!(c.select(10.0), 0);
        assert_eq!(c.select(24.999), 0);
        assert_eq!(c.select(25.0), 1); // boundary → less-detailed
        assert_eq!(c.select(50.0), 1);
        assert_eq!(c.select(80.0), 2);
        assert_eq!(c.select(150.0), 2);
        assert_eq!(c.select(200.0), 3);
        assert_eq!(c.select(10000.0), 3);
    }

    #[test]
    fn single_lod_chain_always_picks_zero() {
        let c = chain(&["only"], &[]);
        assert_eq!(c.select(0.0), 0);
        assert_eq!(c.select(99999.0), 0);
        assert_eq!(c.asset_for_distance(50.0), "only");
    }

    #[test]
    fn asset_for_distance_returns_correct_string() {
        let c = chain(&["a", "b", "c"], &[10.0, 20.0]);
        assert_eq!(c.asset_for_distance(5.0), "a");
        assert_eq!(c.asset_for_distance(15.0), "b");
        assert_eq!(c.asset_for_distance(50.0), "c");
    }

    #[test]
    fn with_table_initializes_lazily() {
        with_table(|t| {
            t.insert("Tree".to_string(), chain(&["a", "b"], &[50.0]));
            assert_eq!(t.get("Tree").unwrap().asset_for_distance(0.0), "a");
            assert_eq!(t.get("Tree").unwrap().asset_for_distance(100.0), "b");
            t.clear();
        });
    }
}
