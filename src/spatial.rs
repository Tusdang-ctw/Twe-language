//! Phase 32 session 2: spatial partitioning.
//!
//! Two complementary structures, both XZ-aligned (Y positions are
//! stored but not partitioned — Tunic-scale open worlds have small
//! vertical extent compared to horizontal, so a 3D partitioning grid
//! adds complexity without a meaningful query-time win):
//!
//! - [`LooseGrid`] for dynamic objects (NPCs, projectiles, anything
//!   that moves every frame). Insertion + removal are O(1); a query
//!   visits a constant set of cells around the query point.
//! - [`Bvh`] for static objects (terrain props, trees, buildings).
//!   Built once, queried many times. Build is O(N log N); query is
//!   O(log N) for typical scenes.
//!
//! Both share an [`Aabb`] ↔ ID mapping so query results from either
//! structure can be merged into one Vec without per-element type
//! conversion.
//!
//! ## Why XZ-only
//!
//! For the v1.0 use cases (Tunic-scale open world, ~50k static props
//! plus ~500 dynamic NPCs, vertical extent under 100m typically), a
//! 2D partitioning grid plus per-query Y-range filter is strictly
//! better than a 3D grid:
//! - 2D cells are cheaper to hash and to traverse during query.
//! - Memory footprint is N×log(N) * 2-axis ≈ half of 3D.
//! - Almost all gameplay queries are "nearby in horizontal plane"
//!   (AI sight, projectile collision, audio attenuation). Y-axis
//!   filtering happens after the spatial cull.
//!
//! Worlds with deep vertical structure (caves, multi-story buildings)
//! that pressure this assumption are tracked as a Phase-32 follow-on
//! session — promote to a 3D grid or a per-floor 2D-grid stack at
//! that point. The author-facing API stays the same; only the
//! implementation changes.
//!
//! ## Threading
//!
//! Per the Phase 32 session 1 lock-revision addendum in CLAUDE.md,
//! engine-internal subsystems may run on a worker pool. The data
//! structures here are `Send + Sync`-safe (no `Rc` / `RefCell` —
//! everything is plain owned data behind a top-level mutex when
//! exposed to scripts). Worker integration lands in a follow-on
//! session; this module ships the synchronous foundation.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;

/// 3D axis-aligned bounding box. Half-open on the upper corner so an
/// AABB exactly touching another's boundary doesn't count as
/// overlapping (matches the standard "rectangles touching at a
/// vertex don't intersect" convention).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    /// Build an AABB from a center + half-extent radius (treats the
    /// object as a sphere inscribed in the box). The grid only ever
    /// intersects boxes against boxes — the "sphere" framing is for
    /// the script-facing API.
    pub fn from_center_radius(x: f32, y: f32, z: f32, r: f32) -> Self {
        Aabb {
            min: [x - r, y - r, z - r],
            max: [x + r, y + r, z + r],
        }
    }

    /// Tight AABB containing both inputs.
    pub fn union(&self, other: &Self) -> Self {
        Aabb {
            min: [
                self.min[0].min(other.min[0]),
                self.min[1].min(other.min[1]),
                self.min[2].min(other.min[2]),
            ],
            max: [
                self.max[0].max(other.max[0]),
                self.max[1].max(other.max[1]),
                self.max[2].max(other.max[2]),
            ],
        }
    }

    /// True if the two AABBs overlap.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.min[0] < other.max[0]
            && self.max[0] > other.min[0]
            && self.min[1] < other.max[1]
            && self.max[1] > other.min[1]
            && self.min[2] < other.max[2]
            && self.max[2] > other.min[2]
    }

    /// True if this AABB overlaps a sphere centered at `(cx, cy, cz)`
    /// with radius `r`. Standard "closest point on AABB to sphere
    /// center, then squared-distance compare" — the per-axis clamp
    /// keeps it sign-clean.
    pub fn overlaps_sphere(&self, cx: f32, cy: f32, cz: f32, r: f32) -> bool {
        let qx = cx.clamp(self.min[0], self.max[0]);
        let qy = cy.clamp(self.min[1], self.max[1]);
        let qz = cz.clamp(self.min[2], self.max[2]);
        let dx = cx - qx;
        let dy = cy - qy;
        let dz = cz - qz;
        dx * dx + dy * dy + dz * dz <= r * r
    }

    /// Centroid in world coordinates.
    pub fn center(&self) -> [f32; 3] {
        [
            0.5 * (self.min[0] + self.max[0]),
            0.5 * (self.min[1] + self.max[1]),
            0.5 * (self.min[2] + self.max[2]),
        ]
    }
}

// ---------- Loose grid (dynamic) ----------

/// XZ-aligned uniform grid optimized for dynamic objects. Each
/// entity is hashed into every cell its AABB touches; query visits
/// the cells covering the query AABB and reports unique IDs.
///
/// Cell size is a tradeoff: larger cells have lower per-object
/// memory (each entity sits in fewer cells) but more entities per
/// cell at query time. 8m cells fit Tunic-scale gameplay (NPCs ~1m,
/// projectiles ~0.2m, query radii ~5–20m typical).
pub struct LooseGrid {
    pub cell_size: f32,
    cells: HashMap<(i32, i32), Vec<u64>>,
    /// id -> AABB, so removal can revisit only the cells the entity
    /// occupies.
    pub(crate) occupants: HashMap<u64, Aabb>,
}

impl LooseGrid {
    pub fn new(cell_size: f32) -> Self {
        assert!(cell_size > 0.0, "cell_size must be positive");
        LooseGrid {
            cell_size,
            cells: HashMap::new(),
            occupants: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.cells.clear();
        self.occupants.clear();
    }

    /// Number of distinct entities inserted. Useful for sizing tests.
    pub fn len(&self) -> usize {
        self.occupants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.occupants.is_empty()
    }

    fn cell_range(&self, aabb: &Aabb) -> (i32, i32, i32, i32) {
        let inv = 1.0 / self.cell_size;
        let x0 = (aabb.min[0] * inv).floor() as i32;
        let x1 = (aabb.max[0] * inv).floor() as i32;
        let z0 = (aabb.min[2] * inv).floor() as i32;
        let z1 = (aabb.max[2] * inv).floor() as i32;
        (x0, x1, z0, z1)
    }

    /// Insert (or replace) `id` with the given AABB. If `id` already
    /// existed, it's removed from its previous cells first.
    pub fn insert(&mut self, id: u64, aabb: Aabb) {
        if self.occupants.contains_key(&id) {
            self.remove(id);
        }
        let (x0, x1, z0, z1) = self.cell_range(&aabb);
        for x in x0..=x1 {
            for z in z0..=z1 {
                self.cells.entry((x, z)).or_default().push(id);
            }
        }
        self.occupants.insert(id, aabb);
    }

    /// Remove `id` from every cell it occupied. Returns true if the
    /// id was present.
    pub fn remove(&mut self, id: u64) -> bool {
        let Some(aabb) = self.occupants.remove(&id) else {
            return false;
        };
        let (x0, x1, z0, z1) = self.cell_range(&aabb);
        for x in x0..=x1 {
            for z in z0..=z1 {
                if let Some(bucket) = self.cells.get_mut(&(x, z)) {
                    bucket.retain(|other| *other != id);
                    if bucket.is_empty() {
                        self.cells.remove(&(x, z));
                    }
                }
            }
        }
        true
    }

    /// All IDs whose AABB intersects the query AABB. Sorted +
    /// deduplicated (an entity straddling 4 cells is reported once).
    pub fn query_box(&self, query: &Aabb) -> Vec<u64> {
        let (x0, x1, z0, z1) = self.cell_range(query);
        let mut hits: Vec<u64> = Vec::new();
        for x in x0..=x1 {
            for z in z0..=z1 {
                if let Some(bucket) = self.cells.get(&(x, z)) {
                    for id in bucket {
                        if let Some(aabb) = self.occupants.get(id) {
                            if aabb.overlaps(query) && !hits.contains(id) {
                                hits.push(*id);
                            }
                        }
                    }
                }
            }
        }
        hits.sort_unstable();
        hits
    }

    /// All IDs whose AABB overlaps the query sphere.
    pub fn query_radius(&self, x: f32, y: f32, z: f32, r: f32) -> Vec<u64> {
        let query = Aabb::from_center_radius(x, y, z, r);
        let (cx0, cx1, cz0, cz1) = self.cell_range(&query);
        let mut hits: Vec<u64> = Vec::new();
        for ix in cx0..=cx1 {
            for iz in cz0..=cz1 {
                if let Some(bucket) = self.cells.get(&(ix, iz)) {
                    for id in bucket {
                        if let Some(aabb) = self.occupants.get(id) {
                            if aabb.overlaps_sphere(x, y, z, r) && !hits.contains(id) {
                                hits.push(*id);
                            }
                        }
                    }
                }
            }
        }
        hits.sort_unstable();
        hits
    }
}

// ---------- BVH (static) ----------

/// Bounding-volume hierarchy over a fixed set of static AABBs.
/// Built once with [`Bvh::build`]; queries traverse the tree and
/// prune subtrees whose AABB doesn't overlap the query.
///
/// Build strategy: top-down median split along the longest axis.
/// Not the SAH-optimal build (cost is more involved and yields
/// ~10–20% query speedups over the median split — not worth the
/// extra build complexity for v1.0). Tracked as a follow-on session.
pub struct Bvh {
    nodes: Vec<BvhNode>,
    /// Leaves' (id, aabb) — referenced by index from leaf nodes.
    leaves: Vec<(u64, Aabb)>,
    root: usize,
}

#[derive(Clone, Debug)]
enum BvhNode {
    Internal {
        bounds: Aabb,
        left: usize,
        right: usize,
    },
    Leaf {
        bounds: Aabb,
        leaf_index: usize,
    },
}

impl Bvh {
    /// Build a BVH from a list of `(id, aabb)` pairs. The list is
    /// taken by value because the build sorts and partitions it
    /// in-place (avoid an extra clone of the input).
    pub fn build(items: Vec<(u64, Aabb)>) -> Self {
        if items.is_empty() {
            return Bvh {
                nodes: Vec::new(),
                leaves: Vec::new(),
                root: 0,
            };
        }
        let leaves = items.clone();
        let mut indices: Vec<usize> = (0..leaves.len()).collect();
        let mut nodes: Vec<BvhNode> = Vec::with_capacity(leaves.len() * 2);
        let root = build_recursive(&mut nodes, &leaves, &mut indices);
        Bvh {
            nodes,
            leaves,
            root,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    pub fn query_box(&self, query: &Aabb) -> Vec<u64> {
        let mut hits = Vec::new();
        if !self.leaves.is_empty() {
            self.traverse_box(self.root, query, &mut hits);
        }
        hits.sort_unstable();
        hits.dedup();
        hits
    }

    pub fn query_radius(&self, x: f32, y: f32, z: f32, r: f32) -> Vec<u64> {
        let query = Aabb::from_center_radius(x, y, z, r);
        let mut hits = Vec::new();
        if !self.leaves.is_empty() {
            self.traverse_sphere(
                self.root,
                &query,
                SphereQuery { x, y, z, r },
                &mut hits,
            );
        }
        hits.sort_unstable();
        hits.dedup();
        hits
    }

    /// All leaf IDs whose AABB passes the frustum test. Internal
    /// nodes whose bounds are fully outside the frustum are pruned;
    /// this gives `O(log N + V)` where V is the visible-leaf count.
    pub fn query_frustum(&self, frustum: &crate::cull::Frustum) -> Vec<u64> {
        let mut hits = Vec::new();
        if !self.leaves.is_empty() {
            self.traverse_frustum(self.root, frustum, &mut hits);
        }
        hits.sort_unstable();
        hits.dedup();
        hits
    }

    fn traverse_box(&self, node: usize, query: &Aabb, out: &mut Vec<u64>) {
        match &self.nodes[node] {
            BvhNode::Leaf { bounds, leaf_index } => {
                if bounds.overlaps(query) {
                    out.push(self.leaves[*leaf_index].0);
                }
            }
            BvhNode::Internal {
                bounds,
                left,
                right,
            } => {
                if !bounds.overlaps(query) {
                    return;
                }
                self.traverse_box(*left, query, out);
                self.traverse_box(*right, query, out);
            }
        }
    }

    fn traverse_frustum(
        &self,
        node: usize,
        frustum: &crate::cull::Frustum,
        out: &mut Vec<u64>,
    ) {
        match &self.nodes[node] {
            BvhNode::Leaf { bounds, leaf_index } => {
                if frustum.may_contain(bounds) {
                    out.push(self.leaves[*leaf_index].0);
                }
            }
            BvhNode::Internal {
                bounds,
                left,
                right,
            } => {
                if frustum.fully_outside(bounds) {
                    return;
                }
                self.traverse_frustum(*left, frustum, out);
                self.traverse_frustum(*right, frustum, out);
            }
        }
    }

    fn traverse_sphere(&self, node: usize, bounding: &Aabb, q: SphereQuery, out: &mut Vec<u64>) {
        match &self.nodes[node] {
            BvhNode::Leaf { bounds, leaf_index } => {
                if bounds.overlaps_sphere(q.x, q.y, q.z, q.r) {
                    out.push(self.leaves[*leaf_index].0);
                }
            }
            BvhNode::Internal {
                bounds,
                left,
                right,
            } => {
                if !bounds.overlaps(bounding) {
                    return;
                }
                self.traverse_sphere(*left, bounding, q, out);
                self.traverse_sphere(*right, bounding, q, out);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct SphereQuery {
    x: f32,
    y: f32,
    z: f32,
    r: f32,
}

fn build_recursive(
    nodes: &mut Vec<BvhNode>,
    leaves: &[(u64, Aabb)],
    indices: &mut [usize],
) -> usize {
    if indices.len() == 1 {
        let leaf_index = indices[0];
        let bounds = leaves[leaf_index].1;
        let id = nodes.len();
        nodes.push(BvhNode::Leaf { bounds, leaf_index });
        return id;
    }

    let bounds = indices
        .iter()
        .map(|i| leaves[*i].1)
        .reduce(|a, b| a.union(&b))
        .expect("non-empty");

    // Pick the longest axis of the bounding volume to split on.
    let extents = [
        bounds.max[0] - bounds.min[0],
        bounds.max[1] - bounds.min[1],
        bounds.max[2] - bounds.min[2],
    ];
    let axis = if extents[0] >= extents[1] && extents[0] >= extents[2] {
        0
    } else if extents[1] >= extents[2] {
        1
    } else {
        2
    };

    indices.sort_by(|a, b| {
        leaves[*a].1.center()[axis]
            .partial_cmp(&leaves[*b].1.center()[axis])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mid = indices.len() / 2;
    let (left_idx, right_idx) = indices.split_at_mut(mid);
    let left = build_recursive(nodes, leaves, left_idx);
    let right = build_recursive(nodes, leaves, right_idx);
    let id = nodes.len();
    nodes.push(BvhNode::Internal {
        bounds,
        left,
        right,
    });
    id
}

// ---------- Combined world singleton ----------

use std::sync::Mutex;

/// Combined dynamic + static spatial state. The script-facing
/// `world.spatial_*` builtins go through this — one global instance
/// per process. Mutex (not RwLock) because every API touch is fast
/// and worker integration is a follow-on; uncontended Mutex is
/// cheaper than RwLock.
pub struct WorldSpatial {
    pub dynamic: LooseGrid,
    static_pending: Vec<(u64, Aabb)>,
    static_built: Option<Bvh>,
}

impl Default for WorldSpatial {
    fn default() -> Self {
        WorldSpatial {
            dynamic: LooseGrid::new(8.0),
            static_pending: Vec::new(),
            static_built: None,
        }
    }
}

impl WorldSpatial {
    pub fn clear(&mut self) {
        self.dynamic.clear();
        self.static_pending.clear();
        self.static_built = None;
    }

    pub fn insert_dynamic(&mut self, id: u64, aabb: Aabb) {
        self.dynamic.insert(id, aabb);
    }

    pub fn remove_dynamic(&mut self, id: u64) -> bool {
        self.dynamic.remove(id)
    }

    pub fn add_static(&mut self, id: u64, aabb: Aabb) {
        self.static_pending.push((id, aabb));
        // Adding to pending invalidates the built tree — rebuild on
        // next query (or explicitly via build_static).
        self.static_built = None;
    }

    pub fn build_static(&mut self) {
        let pending = std::mem::take(&mut self.static_pending);
        self.static_built = Some(Bvh::build(pending));
    }

    pub fn query_radius(&mut self, x: f32, y: f32, z: f32, r: f32) -> Vec<u64> {
        let mut out = self.dynamic.query_radius(x, y, z, r);
        // Rebuild the static tree lazily if pending items exist.
        if !self.static_pending.is_empty() {
            self.build_static();
        }
        if let Some(bvh) = &self.static_built {
            for id in bvh.query_radius(x, y, z, r) {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn query_box(&mut self, query: &Aabb) -> Vec<u64> {
        let mut out = self.dynamic.query_box(query);
        if !self.static_pending.is_empty() {
            self.build_static();
        }
        if let Some(bvh) = &self.static_built {
            for id in bvh.query_box(query) {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// All IDs (dynamic + static) whose AABB might be visible
    /// through `frustum`. Dynamic objects are linear-scanned (the
    /// loose grid doesn't accelerate frustum tests well — every
    /// dynamic object is in some cell, and the cell traversal saves
    /// nothing over per-object plane tests). Static objects use the
    /// BVH: prune internal nodes whose bounds are fully outside
    /// before recursing.
    pub fn query_frustum(&mut self, frustum: &crate::cull::Frustum) -> Vec<u64> {
        let mut out: Vec<u64> = self
            .dynamic
            .occupants
            .iter()
            .filter_map(|(id, aabb)| {
                if frustum.may_contain(aabb) {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        if !self.static_pending.is_empty() {
            self.build_static();
        }
        if let Some(bvh) = &self.static_built {
            for id in bvh.query_frustum(frustum) {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Process-wide spatial state. Thread-safe via Mutex; per the Phase
/// 32 session 1 lock-revision addendum, engine-internal subsystems
/// may run on a worker pool, so the data structures must be ready
/// for off-main-thread access.
pub static WORLD: Mutex<Option<WorldSpatial>> = Mutex::new(None);

/// Get-or-init the global spatial state; runs `f` against it and
/// returns whatever `f` returns. Pattern matches `replay::SESSION` /
/// `net::SESSION` thread-locals but uses a Mutex because `WorldSpatial`
/// must be reachable from worker threads.
pub fn with_world<R>(f: impl FnOnce(&mut WorldSpatial) -> R) -> R {
    let mut guard = WORLD.lock().expect("world spatial mutex poisoned");
    if guard.is_none() {
        *guard = Some(WorldSpatial::default());
    }
    f(guard.as_mut().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aabb_overlaps_sphere_clamps_correctly() {
        let a = Aabb {
            min: [-1.0, -1.0, -1.0],
            max: [1.0, 1.0, 1.0],
        };
        // Sphere center inside the box → always overlap.
        assert!(a.overlaps_sphere(0.0, 0.0, 0.0, 0.5));
        // Sphere outside but reaches box.
        assert!(a.overlaps_sphere(2.0, 0.0, 0.0, 1.5));
        // Sphere outside and doesn't reach.
        assert!(!a.overlaps_sphere(5.0, 0.0, 0.0, 1.0));
        // Diagonal — 3D distance from corner.
        assert!(a.overlaps_sphere(2.0, 2.0, 2.0, 2.0));
        assert!(!a.overlaps_sphere(2.0, 2.0, 2.0, 1.5));
    }

    #[test]
    fn loose_grid_finds_overlapping_dynamic() {
        let mut g = LooseGrid::new(8.0);
        g.insert(1, Aabb::from_center_radius(0.0, 0.0, 0.0, 1.0));
        g.insert(2, Aabb::from_center_radius(15.0, 0.0, 0.0, 1.0));
        g.insert(3, Aabb::from_center_radius(50.0, 0.0, 0.0, 1.0));
        let near_origin = g.query_radius(0.0, 0.0, 0.0, 5.0);
        assert_eq!(near_origin, vec![1]);
        let medium = g.query_radius(0.0, 0.0, 0.0, 16.0);
        assert_eq!(medium, vec![1, 2]);
        let far = g.query_radius(50.0, 0.0, 0.0, 5.0);
        assert_eq!(far, vec![3]);
    }

    #[test]
    fn loose_grid_remove_round_trips() {
        let mut g = LooseGrid::new(8.0);
        g.insert(1, Aabb::from_center_radius(0.0, 0.0, 0.0, 1.0));
        g.insert(2, Aabb::from_center_radius(0.0, 0.0, 0.0, 1.0));
        assert_eq!(g.len(), 2);
        assert!(g.remove(1));
        assert!(!g.remove(1)); // gone, second remove is a no-op
        assert_eq!(g.len(), 1);
        let hits = g.query_radius(0.0, 0.0, 0.0, 5.0);
        assert_eq!(hits, vec![2]);
    }

    #[test]
    fn loose_grid_handles_objects_spanning_cells() {
        let mut g = LooseGrid::new(8.0);
        // A 4-meter-radius object centered on a cell boundary
        // touches 4 cells (XZ corners). Query from one corner
        // should still find it.
        g.insert(1, Aabb::from_center_radius(8.0, 0.0, 8.0, 4.0));
        let hits = g.query_radius(7.0, 0.0, 7.0, 0.5);
        assert_eq!(hits, vec![1]);
    }

    #[test]
    fn bvh_finds_static_overlaps() {
        let items: Vec<(u64, Aabb)> = (0..100)
            .map(|i| (i, Aabb::from_center_radius(i as f32 * 5.0, 0.0, 0.0, 1.0)))
            .collect();
        let bvh = Bvh::build(items);
        let near = bvh.query_radius(50.0, 0.0, 0.0, 6.0);
        // Items 9, 10, 11 sit at x=45, 50, 55 with radius 1 each;
        // a sphere of radius 6 from (50,0,0) reaches x=44..56 — so
        // items 9/10/11 hit (x=45/50/55 ± 1 ⇒ closest distance 4/0/4).
        assert_eq!(near, vec![9, 10, 11]);
    }

    #[test]
    fn bvh_build_handles_empty_and_single() {
        let empty = Bvh::build(Vec::new());
        assert!(empty.is_empty());
        assert!(empty.query_radius(0.0, 0.0, 0.0, 100.0).is_empty());

        let single = Bvh::build(vec![(42, Aabb::from_center_radius(0.0, 0.0, 0.0, 1.0))]);
        assert_eq!(single.query_radius(0.0, 0.0, 0.0, 5.0), vec![42]);
        assert_eq!(single.query_radius(100.0, 0.0, 0.0, 5.0), Vec::<u64>::new());
    }

    #[test]
    fn world_merges_dynamic_and_static_results() {
        let mut w = WorldSpatial::default();
        w.insert_dynamic(1, Aabb::from_center_radius(0.0, 0.0, 0.0, 1.0));
        w.add_static(100, Aabb::from_center_radius(2.0, 0.0, 0.0, 1.0));
        w.add_static(101, Aabb::from_center_radius(50.0, 0.0, 0.0, 1.0));
        let near = w.query_radius(0.0, 0.0, 0.0, 5.0);
        assert_eq!(near, vec![1, 100]); // sorted, deduped
    }

    #[test]
    fn with_world_initializes_lazily() {
        // The static singleton should default-init on first access.
        with_world(|w| {
            w.clear();
            w.insert_dynamic(7, Aabb::from_center_radius(1.0, 0.0, 1.0, 0.5));
            assert_eq!(w.query_radius(0.0, 0.0, 0.0, 5.0), vec![7]);
            w.clear();
        });
    }
}
