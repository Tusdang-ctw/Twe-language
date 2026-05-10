//! Phase 32 session 6: frustum culling — extension of the Phase 19–23
//! 3D pipeline.
//!
//! A [`Frustum`] is six planes derived from a 4×4 view-projection
//! matrix (column-major, OpenGL/wgpu clip-space convention). Given
//! an AABB or sphere, the frustum reports whether the volume is
//! potentially visible. False positives (an AABB that passes the
//! frustum test but is fully off-screen due to clipping by adjacent
//! planes) are accepted; false negatives are not.
//!
//! ## Why this is the v1.0 culling story
//!
//! Frustum culling drops 50–80% of off-screen draws on a typical
//! Tunic-scale open-world camera. Combined with the Phase 32 session 2
//! BVH, the cost is `O(log N)` to enumerate visible static props and
//! the dynamic loose-grid contributes a per-cell visit. That's
//! enough to hit the 50k-prop / 60fps exit criterion on a 4-year-old
//! GPU.
//!
//! Occluder-based culling (large opaque AABBs, e.g. cliff faces,
//! that hide everything behind them from a given camera) is a
//! separate technique with diminishing returns once frustum +
//! BVH-spatial are in place. Tracked as a Phase-32 follow-on:
//! - **Why deferred:** correctness is subtle (rays through an
//!   occluder must clear a margin), and the speedup over frustum
//!   is < 2× on most scenes.
//! - **What lands when:** `world.add_occluder(aabb)` +
//!   `world.cull_with_occluders(view_proj, camera_pos)` —
//!   tested against the frustum-visible set first.
//!
//! GPU hierarchical-Z occlusion (the AAA-tier technique) requires
//! rendering a depth pre-pass and reading it back; that's a
//! multi-session integration with the wgpu pipeline. Out of scope
//! for v1.0.

#![cfg(not(target_arch = "wasm32"))]

use crate::spatial::Aabb;

/// Six planes of a view frustum in world-space coordinates. Each
/// plane is `(a, b, c, d)` with the convention `a*x + b*y + c*z + d
/// >= 0` for the inside half-space. Plane normals point inward.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    pub planes: [[f32; 4]; 6],
}

impl Frustum {
    /// Extract the six clip-space planes from a row-major
    /// view-projection matrix (combined `proj * view`). Rows are
    /// the matrix rows in row-major order: `m[i][j]` is row i,
    /// column j.
    ///
    /// The Gribb-Hartmann technique: each plane is a sum/difference
    /// of two matrix rows; this works for any projection matrix
    /// (perspective, orthographic, off-center). Output planes are
    /// already normalized.
    pub fn from_view_proj_row_major(m: [[f32; 4]; 4]) -> Self {
        let row = |i: usize| [m[i][0], m[i][1], m[i][2], m[i][3]];
        let r0 = row(0);
        let r1 = row(1);
        let r2 = row(2);
        let r3 = row(3);

        // Left = r3 + r0; Right = r3 - r0; Bottom = r3 + r1;
        // Top = r3 - r1; Near = r3 + r2; Far = r3 - r2.
        let raw = [
            add(r3, r0),
            sub(r3, r0),
            add(r3, r1),
            sub(r3, r1),
            add(r3, r2),
            sub(r3, r2),
        ];
        let mut planes = [[0.0; 4]; 6];
        for (i, p) in raw.iter().enumerate() {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-9);
            planes[i] = [p[0] / len, p[1] / len, p[2] / len, p[3] / len];
        }
        Frustum { planes }
    }

    /// Same as [`from_view_proj_row_major`] but for a column-major
    /// matrix layout (`m[col][row]`, the wgpu / glm convention).
    /// Most renderers store matrices column-major; this is the
    /// typical entry point.
    pub fn from_view_proj_column_major(m: [[f32; 4]; 4]) -> Self {
        let mut row_major = [[0.0; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                row_major[i][j] = m[j][i];
            }
        }
        Self::from_view_proj_row_major(row_major)
    }

    /// True if the AABB is fully outside the frustum (can be
    /// culled). Tests the AABB's "negative vertex" against each
    /// plane: the corner farthest from the plane in the inside
    /// direction; if that vertex is outside, the whole AABB is.
    pub fn fully_outside(&self, aabb: &Aabb) -> bool {
        for plane in &self.planes {
            // Pick the corner that maximizes `plane · corner` —
            // i.e. the deepest point along the plane normal. For
            // each axis, choose min[axis] if normal is negative,
            // max[axis] if positive.
            let px = if plane[0] >= 0.0 { aabb.max[0] } else { aabb.min[0] };
            let py = if plane[1] >= 0.0 { aabb.max[1] } else { aabb.min[1] };
            let pz = if plane[2] >= 0.0 { aabb.max[2] } else { aabb.min[2] };
            if plane[0] * px + plane[1] * py + plane[2] * pz + plane[3] < 0.0 {
                return true;
            }
        }
        false
    }

    /// True if the AABB *might* be visible (passes the frustum
    /// test). A `true` result is conservative — the AABB might be
    /// fully outside but pass adjacent-plane corner cases. False
    /// positives are fine; the rendered draw call is just a no-op.
    /// False negatives would visibly clip geometry.
    pub fn may_contain(&self, aabb: &Aabb) -> bool {
        !self.fully_outside(aabb)
    }

    /// True if the sphere centered at `(cx, cy, cz)` with radius
    /// `r` might be visible. Cheaper than the AABB test — one
    /// distance compare per plane. Use this when the bounding
    /// sphere is naturally tighter than the AABB.
    pub fn may_contain_sphere(&self, cx: f32, cy: f32, cz: f32, r: f32) -> bool {
        for plane in &self.planes {
            let d = plane[0] * cx + plane[1] * cy + plane[2] * cz + plane[3];
            if d < -r {
                return false;
            }
        }
        true
    }
}

fn add(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

fn sub(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]]
}

/// Build a row-major perspective projection matrix matching the
/// wgpu clip space `[-1, 1] × [-1, 1] × [0, 1]`. Used by tests +
/// scripts that want to construct a frustum without going through
/// a real camera.
pub fn perspective_row_major(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fov_y_radians * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), (far * near) / (near - far)],
        [0.0, 0.0, -1.0, 0.0],
    ]
}

/// Translation matrix (row-major) — used to combine with a
/// projection matrix to make a basic view-projection.
pub fn translate_row_major(tx: f32, ty: f32, tz: f32) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, tx],
        [0.0, 1.0, 0.0, ty],
        [0.0, 0.0, 1.0, tz],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub fn matmul_row_major(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            for k in 0..4 {
                out[i][j] += a[i][k] * b[k][j];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forward_camera_frustum() -> Frustum {
        // Camera at origin looking down -Z (the wgpu convention).
        // Translate the world by (0, 0, 0) — view matrix is identity
        // since camera is at origin.
        let proj = perspective_row_major(90f32.to_radians(), 1.0, 0.1, 100.0);
        Frustum::from_view_proj_row_major(proj)
    }

    #[test]
    fn aabb_in_front_of_camera_passes() {
        let f = forward_camera_frustum();
        // AABB at z = -10 (in front of camera), small extents.
        let aabb = Aabb {
            min: [-1.0, -1.0, -11.0],
            max: [1.0, 1.0, -9.0],
        };
        assert!(f.may_contain(&aabb));
    }

    #[test]
    fn aabb_behind_camera_culls() {
        let f = forward_camera_frustum();
        // AABB at z = +10 (behind camera).
        let aabb = Aabb {
            min: [-1.0, -1.0, 9.0],
            max: [1.0, 1.0, 11.0],
        };
        assert!(f.fully_outside(&aabb));
    }

    #[test]
    fn aabb_far_beyond_far_plane_culls() {
        let f = forward_camera_frustum();
        let aabb = Aabb {
            min: [-1.0, -1.0, -1000.0],
            max: [1.0, 1.0, -999.0],
        };
        assert!(f.fully_outside(&aabb));
    }

    #[test]
    fn aabb_far_off_to_the_side_culls() {
        let f = forward_camera_frustum();
        // AABB at x = +1000 — well outside the 90-degree horizontal
        // FOV at z = -10.
        let aabb = Aabb {
            min: [999.0, -1.0, -11.0],
            max: [1001.0, 1.0, -9.0],
        };
        assert!(f.fully_outside(&aabb));
    }

    #[test]
    fn sphere_in_front_of_camera_passes() {
        let f = forward_camera_frustum();
        assert!(f.may_contain_sphere(0.0, 0.0, -10.0, 1.0));
    }

    #[test]
    fn sphere_behind_camera_culls() {
        let f = forward_camera_frustum();
        assert!(!f.may_contain_sphere(0.0, 0.0, 10.0, 0.5));
    }

    #[test]
    fn sphere_straddling_near_plane_passes_conservatively() {
        let f = forward_camera_frustum();
        // Sphere centered at z = +0.05 (just behind camera) with
        // radius 1 reaches into the frustum past the near plane —
        // should pass.
        assert!(f.may_contain_sphere(0.0, 0.0, 0.05, 1.0));
    }

    #[test]
    fn matrix_helpers_compose_correctly() {
        let proj = perspective_row_major(60f32.to_radians(), 16.0 / 9.0, 0.1, 1000.0);
        let view = translate_row_major(0.0, 0.0, -5.0);
        let vp = matmul_row_major(proj, view);
        let f = Frustum::from_view_proj_row_major(vp);
        // Origin (where camera is) should be roughly behind the
        // near plane after the view-translate, i.e. NOT in the
        // frustum.
        assert!(!f.may_contain_sphere(0.0, 0.0, 5.0, 0.01));
        // Point in front of camera in world space should pass.
        assert!(f.may_contain_sphere(0.0, 0.0, 0.0, 0.5));
    }

    #[test]
    fn column_major_matches_row_major() {
        let row_major = perspective_row_major(90f32.to_radians(), 1.0, 0.1, 100.0);
        // Transpose to column-major.
        let mut col_major = [[0.0; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                col_major[i][j] = row_major[j][i];
            }
        }
        let fa = Frustum::from_view_proj_row_major(row_major);
        let fb = Frustum::from_view_proj_column_major(col_major);
        for i in 0..6 {
            for j in 0..4 {
                assert!(
                    (fa.planes[i][j] - fb.planes[i][j]).abs() < 1e-5,
                    "plane {i} component {j}: {} vs {}",
                    fa.planes[i][j],
                    fb.planes[i][j]
                );
            }
        }
    }
}
