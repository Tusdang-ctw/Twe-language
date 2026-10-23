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
    /// Full-resolution pixels per ambient-occlusion texel per axis:
    /// 2 (half resolution) or 4 (quarter).
    pub ao_downsample: u32,
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
            ao_downsample: 2,
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
            // shadowed lights, quarter-resolution AO, no SSR /
            // volumetrics / TAA, 85 % scale.
            Tier::Medium => Quality {
                render_scale: 0.85,
                ao_downsample: 4,
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
                ao_downsample: 4,
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

/// Low-end track (G5): the tier a GPU starts on before any frame has
/// been timed. Discrete GPUs start on `Ultra`; integrated, virtual and
/// unknown ones (browsers don't report the kind) on `High`; software
/// rasterisers on `Low`. The governor then moves from there.
pub fn starting_tier(device: wgpu::DeviceType) -> Tier {
    match device {
        wgpu::DeviceType::DiscreteGpu => Tier::Ultra,
        wgpu::DeviceType::Cpu => Tier::Low,
        _ => Tier::High,
    }
}

/// Low-end track (G5): dynamic resolution and tier stepping, for games
/// that don't choose a tier.
///
/// It reads only the wall-clock interval between frames, the one signal
/// every host has (a browser can't read GPU timestamps without stalling).
/// With vsync the interval sits at the display's refresh period while
/// the GPU keeps up, so the governor can't see headroom directly:
/// - **Down, fast.** When the smoothed interval stays over 1.2× the
///   target for half a second, the render scale drops 0.1. Below 0.5 of
///   the tier's own scale it steps down a tier.
/// - **Up, slowly.** After three seconds at the target it probes 0.05
///   up. It never probes within ten seconds of a step down, so it
///   doesn't oscillate on a scene that only just fits.
///
/// Off in `?capture` mode and in tests and benchmarks: hosts create it
/// only for live, interactive frames.
#[derive(Clone, Debug)]
pub struct AutoQuality {
    /// The tier it started on; it never probes above this.
    ceiling: Tier,
    tier: Tier,
    /// Fraction of the tier's own render scale (0.5 – 1.0).
    scale: f32,
    /// The frame interval it aims for, in seconds.
    target: f32,
    ema: f32,
    over: f32,
    under: f32,
    since_drop: f32,
    /// Seconds left in the post-change settle window (see `SETTLE_SECS`).
    settle: f32,
}

/// Low-end track: how long to ignore load after a budget change. Changing
/// the budget reallocates every full-size GPU target, and some browsers
/// stall for a moment while they free the old ones. That transient says
/// nothing about steady-state cost, so reading it as real load would
/// cascade the budget to its floor and recover only slowly. Tunable:
/// confirm against a `chrome://tracing` capture across a switch.
const SETTLE_SECS: f32 = 1.0;

impl AutoQuality {
    /// Aim for `target_fps` (60 unless the display is known to differ).
    pub fn new(start: Tier, target_fps: f32) -> AutoQuality {
        let target = 1.0 / target_fps.clamp(20.0, 240.0);
        AutoQuality {
            ceiling: start,
            tier: start,
            scale: 1.0,
            target,
            ema: target,
            over: 0.0,
            under: 0.0,
            since_drop: f32::INFINITY,
            settle: 0.0,
        }
    }

    /// The budget for the next frame.
    pub fn quality(&self) -> Quality {
        let mut q = Quality::tier(self.tier);
        q.render_scale *= self.scale;
        q
    }

    /// Feed the interval since the last frame (seconds). Returns true when
    /// the budget changed.
    pub fn update(&mut self, interval: f32) -> bool {
        // Ignore hitches (a tab switch, a debugger pause, an asset
        // load): they say nothing about the GPU. A game that really runs
        // at 4 fps or slower is left alone, which is acceptable: no tier
        // would rescue it.
        if !(0.0..=0.25).contains(&interval) {
            return false;
        }
        let dt = interval;
        // Low-end track: inside the settle window after a budget change,
        // ignore load entirely — the realloc transient says nothing about
        // steady-state cost. Hold the average at the target so the window
        // ends on a clean slate rather than the spike it was absorbing.
        if self.settle > 0.0 {
            self.settle -= dt;
            self.ema = self.target;
            self.over = 0.0;
            self.under = 0.0;
            return false;
        }
        self.ema += (dt - self.ema) * 0.1;
        self.since_drop += dt;
        if self.ema > self.target * 1.2 {
            self.over += dt;
            self.under = 0.0;
        } else if self.ema < self.target * 1.05 {
            self.under += dt;
            self.over = 0.0;
        } else {
            self.over = 0.0;
            self.under = 0.0;
        }
        if self.over >= 0.5 {
            self.over = 0.0;
            self.since_drop = 0.0;
            // Start the next judgement from the target, not the bad
            // average this step is answering.
            self.ema = self.target;
            if self.scale > 0.55 {
                self.scale = (self.scale - 0.1).max(0.5);
            } else if let Some(lower) = self.step(-1) {
                self.tier = lower;
                self.scale = 1.0;
            } else {
                return false;
            }
            self.settle = SETTLE_SECS;
            return true;
        }
        if self.under >= 3.0 && self.since_drop >= 10.0 {
            self.under = 0.0;
            if self.scale < 1.0 {
                self.scale = (self.scale + 0.05).min(1.0);
                self.settle = SETTLE_SECS;
                return true;
            }
            if self.tier < self.ceiling {
                if let Some(higher) = self.step(1) {
                    self.tier = higher;
                    self.scale = 0.5;
                    self.settle = SETTLE_SECS;
                    return true;
                }
            }
        }
        false
    }

    fn step(&self, by: i32) -> Option<Tier> {
        let i = Tier::ALL.iter().position(|&t| t == self.tier)? as i32 + by;
        Tier::ALL.get(usize::try_from(i).ok()?).copied()
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

    /// Run `secs` seconds of frames `interval` apart.
    fn run(g: &mut AutoQuality, interval: f32, secs: f32) {
        let mut t = 0.0;
        while t < secs {
            g.update(interval);
            t += interval;
        }
    }

    #[test]
    fn governor_holds_when_frames_are_on_time() {
        let mut g = AutoQuality::new(Tier::Ultra, 60.0);
        run(&mut g, 1.0 / 60.0, 30.0);
        assert_eq!(g.quality(), Quality::tier(Tier::Ultra));
    }

    #[test]
    fn governor_scales_then_steps_down_when_slow() {
        let mut g = AutoQuality::new(Tier::Ultra, 60.0);
        // 30 fps: twice the target. The scale falls first...
        run(&mut g, 1.0 / 30.0, 1.0);
        assert!(g.quality().render_scale < 1.0);
        assert_eq!(g.quality().tier, Tier::Ultra);
        // ...then whole tiers, down to the floor, and stays there.
        run(&mut g, 1.0 / 30.0, 60.0);
        assert_eq!(g.quality().tier, Tier::Low);
        assert!(g.quality().render_scale >= Quality::tier(Tier::Low).render_scale * 0.5 - 1e-6);
    }

    #[test]
    fn governor_recovers_slowly_and_never_above_its_start() {
        let mut g = AutoQuality::new(Tier::High, 60.0);
        run(&mut g, 1.0 / 30.0, 3.0);
        let dropped = g.quality();
        assert!(dropped.render_scale < Quality::tier(Tier::High).render_scale);
        // Back on time: nothing for ten seconds after the last drop...
        run(&mut g, 1.0 / 60.0, 8.0);
        assert_eq!(g.quality(), dropped);
        // ...then it climbs back, but not past High.
        run(&mut g, 1.0 / 60.0, 300.0);
        assert_eq!(g.quality(), Quality::tier(Tier::High));
    }

    #[test]
    fn settle_window_absorbs_the_post_change_transient() {
        let mut g = AutoQuality::new(Tier::Ultra, 60.0);
        // Drive it just past its first drop.
        run(&mut g, 1.0 / 30.0, 1.0);
        let after_first = g.quality();
        assert!(after_first.render_scale < 1.0);
        // The realloc transient right after a change — a short burst of
        // very slow frames — must not cascade the budget further while the
        // settle window is open.
        for _ in 0..5 {
            g.update(0.1);
        }
        assert_eq!(
            g.quality(),
            after_first,
            "slow frames inside the settle window are ignored"
        );
    }

    #[test]
    fn hitches_alone_change_nothing() {
        let mut g = AutoQuality::new(Tier::Ultra, 60.0);
        for _ in 0..20 {
            run(&mut g, 1.0 / 60.0, 2.0);
            g.update(2.0); // a tab switch
        }
        assert_eq!(g.quality().tier, Tier::Ultra);
    }

    #[test]
    fn starting_tiers() {
        assert_eq!(starting_tier(wgpu::DeviceType::DiscreteGpu), Tier::Ultra);
        assert_eq!(starting_tier(wgpu::DeviceType::IntegratedGpu), Tier::High);
        assert_eq!(starting_tier(wgpu::DeviceType::Other), Tier::High);
        assert_eq!(starting_tier(wgpu::DeviceType::Cpu), Tier::Low);
    }

    #[test]
    fn tiers_are_monotonic() {
        // A higher tier never allows less than a lower one.
        for w in Tier::ALL.windows(2) {
            let (lo, hi) = (Quality::tier(w[0]), Quality::tier(w[1]));
            assert!(hi.render_scale >= lo.render_scale);
            assert!(hi.particle_density >= lo.particle_density);
            assert!(hi.ao_downsample <= lo.ao_downsample);
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
