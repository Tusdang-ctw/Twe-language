//! web3d (low-end track, G2): quality tiers.
//!
//! A tier is a GPU budget: how big the shadow map is, how many point
//! lights cast shadows, which screen-space effects may run, and at what
//! fraction of the window the 3D scene renders. A game asks for effects
//! with `postfx.*` / `light.*`; the tier decides which of those requests
//! the GPU can afford and quietly drops the rest — it is a runtime
//! budget, not an error, so `twec verify` says nothing about it.
//!
//! `Ultra` is exactly the renderer before tiers existed (every request
//! honoured, full resolution, every particle, 4 shadowed point lights),
//! and it is the default — so the image harness and every existing
//! capture are unchanged unless a game or host picks a lower tier.
//!
//! Design: `docs/changes/2026-10-14-web3d-quality-tiers.md`.

use crate::kernel::render::{FogSettings, PostFx};

/// A named budget, lowest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Low,
    Medium,
    High,
    Ultra,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Low, Tier::Medium, Tier::High, Tier::Ultra];

    pub fn name(self) -> &'static str {
        match self {
            Tier::Low => "low",
            Tier::Medium => "medium",
            Tier::High => "high",
            Tier::Ultra => "ultra",
        }
    }

    pub fn parse(name: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.name() == name)
    }
}

/// What a tier allows. Every field is a ceiling on what a game asked
/// for, never a request of its own: a `High` tier doesn't turn bloom on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quality {
    pub tier: Tier,
    /// Fraction of the window (per axis) the 3D scene renders at; the
    /// tonemap pass scales it up to the window. The 2D HUD always draws
    /// at the window's own resolution.
    pub render_scale: f32,
    /// Fraction of each particle emission that is spawned (particles
    /// are cosmetic, so thinning them never changes a replay). Simulation
    /// and drawing cost both follow the live count.
    pub particle_density: f32,
    /// Point / spot lights that may cast shadows at once.
    pub point_shadows: usize,
    pub ao: bool,
    pub ssr: bool,
    pub volumetric_fog: bool,
    pub taa: bool,
    pub dof: bool,
    pub motion_blur: bool,
    pub bloom: bool,
}

impl Quality {
    pub fn tier(tier: Tier) -> Quality {
        let ultra = Quality {
            tier,
            render_scale: 1.0,
            particle_density: 1.0,
            point_shadows: 4,
            ao: true,
            ssr: true,
            volumetric_fog: true,
            taa: true,
            dof: true,
            motion_blur: true,
            bloom: true,
        };
        match tier {
            Tier::Ultra => ultra,
            // High: everything but the two most expensive screen-space
            // effects' worst cases stays; the scene keeps full
            // resolution.
            Tier::High => Quality {
                volumetric_fog: false,
                ..ultra
            },
            // Medium: an integrated GPU at 1080p. Half the particles, two
            // shadowed lights, no SSR / volumetrics / TAA, 85 % scale.
            Tier::Medium => Quality {
                render_scale: 0.85,
                particle_density: 0.5,
                point_shadows: 2,
                ssr: false,
                volumetric_fog: false,
                taa: false,
                motion_blur: false,
                ..ultra
            },
            // Low: weak integrated / mobile GPUs. Sun shadows only, no
            // screen-space effects apart from bloom, a quarter of the
            // particles, 70 % scale.
            Tier::Low => Quality {
                render_scale: 0.7,
                particle_density: 0.25,
                point_shadows: 0,
                ao: false,
                ssr: false,
                volumetric_fog: false,
                taa: false,
                dof: false,
                motion_blur: false,
                ..ultra
            },
        }
    }

    /// The scene's render size for a `width × height` window.
    pub fn scene_size(&self, width: u32, height: u32) -> (u32, u32) {
        let s = self.render_scale.clamp(0.25, 1.0);
        if s >= 1.0 {
            return (width.max(1), height.max(1));
        }
        let scale = |v: u32| ((v as f32 * s).round() as u32).max(1);
        (scale(width), scale(height))
    }

    /// How many of an emission's `count` particles to spawn.
    pub fn particles(&self, count: u32) -> u32 {
        if count == 0 || self.particle_density >= 1.0 {
            return count;
        }
        ((count as f32 * self.particle_density.max(0.0)).ceil() as u32).clamp(1, count)
    }

    /// Drop the effects this tier can't afford from a frame's requests.
    pub fn clamp(&self, post: &mut PostFx, fog: &mut Option<FogSettings>) {
        if !self.ao {
            post.ao = 0.0;
        }
        if !self.ssr {
            post.ssr = 0.0;
        }
        if !self.taa {
            post.taa = false;
        }
        if !self.dof {
            post.dof_focus = 0.0;
        }
        if !self.motion_blur {
            post.motion_blur = 0.0;
        }
        if !self.bloom {
            post.bloom_intensity = 0.0;
        }
        if !self.volumetric_fog {
            if let Some(f) = fog {
                f.volumetric = false;
            }
        }
    }
}

impl Default for Quality {
    fn default() -> Self {
        Quality::tier(Tier::Ultra)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_parse_by_name() {
        for t in Tier::ALL {
            assert_eq!(Tier::parse(t.name()), Some(t));
        }
        assert_eq!(Tier::parse("epic"), None);
    }

    #[test]
    fn ultra_changes_nothing() {
        let q = Quality::default();
        let mut post = PostFx {
            ao: 1.0,
            ssr: 1.0,
            taa: true,
            bloom_intensity: 0.5,
            motion_blur: 0.5,
            dof_focus: 3.0,
            ..PostFx::default()
        };
        let before = format!("{post:?}");
        let mut fog = Some(FogSettings {
            density: 0.1,
            falloff: 0.0,
            color: [1.0; 3],
            volumetric: true,
        });
        q.clamp(&mut post, &mut fog);
        assert_eq!(format!("{post:?}"), before);
        assert!(fog.unwrap().volumetric);
        assert_eq!(q.scene_size(1280, 720), (1280, 720));
    }

    #[test]
    fn low_drops_screen_space_effects_and_scales() {
        let q = Quality::tier(Tier::Low);
        let mut post = PostFx {
            ao: 1.0,
            ssr: 1.0,
            taa: true,
            bloom_intensity: 0.5,
            ..PostFx::default()
        };
        let mut fog = None;
        q.clamp(&mut post, &mut fog);
        assert_eq!(post.ao, 0.0);
        assert_eq!(post.ssr, 0.0);
        assert!(!post.taa);
        assert_eq!(post.bloom_intensity, 0.5, "bloom is a ceiling, kept on low");
        assert_eq!(q.scene_size(1280, 720), (896, 504));
        assert_eq!(q.particles(1000), 250);
        assert_eq!(q.particles(1), 1, "an emission never vanishes entirely");
        assert_eq!(q.particles(0), 0);
    }

    #[test]
    fn tiers_are_monotonic() {
        // A higher tier never allows less than a lower one.
        for w in Tier::ALL.windows(2) {
            let (lo, hi) = (Quality::tier(w[0]), Quality::tier(w[1]));
            assert!(hi.render_scale >= lo.render_scale);
            assert!(hi.particle_density >= lo.particle_density);
            assert!(hi.point_shadows >= lo.point_shadows);
            for (a, b) in [
                (lo.ao, hi.ao),
                (lo.ssr, hi.ssr),
                (lo.volumetric_fog, hi.volumetric_fog),
                (lo.taa, hi.taa),
                (lo.dof, hi.dof),
                (lo.motion_blur, hi.motion_blur),
                (lo.bloom, hi.bloom),
            ] {
                assert!(!a || b);
            }
        }
    }
}
