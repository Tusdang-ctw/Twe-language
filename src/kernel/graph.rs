//! web3d-M7: the render graph.
//!
//! Each frame the renderer *declares* its passes: what each one reads
//! and writes. The graph then decides what actually runs and which GPU
//! textures back the intermediate results:
//!
//! - **Order** is declaration order; a pass that reads a transient
//!   texture nothing has written yet is an error, not a black frame.
//! - **Culling:** only passes that (transitively) feed an *output*
//!   survive. A post effect whose result nobody samples costs nothing.
//! - **Transient textures** (HDR colour, depth, blur chains …) live
//!   from their first to their last use. Two whose lifetimes don't
//!   overlap and whose descriptions match share one allocation, and
//!   the [`TexturePool`] keeps allocations across frames, reallocating
//!   only when the target size or a description changes.
//! - **Imported textures** (the swapchain image, the shadow atlas, a
//!   TAA history buffer) belong to the renderer; the graph only tracks
//!   who touches them.
//!
//! The planner ([`FrameGraph`] → [`Plan`]) is pure data: it knows
//! nothing about wgpu command recording, so it is unit-tested without
//! a GPU. The renderer executes a plan by matching on its own pass
//! type `P`, which keeps pass code free of captured-closure lifetimes.
//! Design: `docs/changes/2026-09-29-web3d-m7-render-graph.md`.
//! (After the FrameGraph talk by Yuriy O'Donnell, GDC 2017.)

use std::fmt;

/// How big a transient texture is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Extent {
    /// The render target's size, divided by `2^shift` (0 = full size).
    Target { shift: u32 },
    /// A fixed size in texels.
    Fixed { width: u32, height: u32 },
}

impl Extent {
    pub const FULL: Extent = Extent::Target { shift: 0 };

    /// Texels for a target of `width × height` (never zero).
    pub fn resolve(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Extent::Target { shift } => ((width >> shift).max(1), (height >> shift).max(1)),
            Extent::Fixed { width, height } => (width.max(1), height.max(1)),
        }
    }
}

/// A transient texture's description. Usage flags are not part of it:
/// the graph derives them from how passes access the texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureDesc {
    pub label: &'static str,
    pub extent: Extent,
    pub format: wgpu::TextureFormat,
    pub mips: u32,
    pub layers: u32,
    pub samples: u32,
}

impl TextureDesc {
    /// A single-mip, single-layer, single-sample 2D texture.
    pub fn new(label: &'static str, extent: Extent, format: wgpu::TextureFormat) -> Self {
        TextureDesc {
            label,
            extent,
            format,
            mips: 1,
            layers: 1,
            samples: 1,
        }
    }

    /// Whether two descriptions can share one allocation (labels aside).
    fn compatible(&self, other: &TextureDesc) -> bool {
        (self.extent, self.format, self.mips, self.layers, self.samples)
            == (other.extent, other.format, other.mips, other.layers, other.samples)
    }
}

/// A texture the graph knows about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Res(u32);

/// How a pass touches a texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Sampled in a shader.
    Sample,
    /// A colour or depth attachment the pass renders into.
    Attach,
    /// Read or written as a storage texture (compute).
    Storage,
}

impl Access {
    fn usage(self) -> wgpu::TextureUsages {
        match self {
            Access::Sample => wgpu::TextureUsages::TEXTURE_BINDING,
            Access::Attach => wgpu::TextureUsages::RENDER_ATTACHMENT,
            Access::Storage => wgpu::TextureUsages::STORAGE_BINDING,
        }
    }
}

enum Kind {
    Transient(TextureDesc),
    /// Owned by the renderer. `output` marks what the frame is *for*
    /// (the swapchain image): passes feeding it are never culled.
    Imported { name: &'static str, output: bool },
}

struct ResourceDecl {
    kind: Kind,
}

struct PassDecl<P> {
    pass: P,
    name: &'static str,
    reads: Vec<(Res, Access)>,
    writes: Vec<(Res, Access)>,
}

/// One frame's declared passes and textures.
pub struct FrameGraph<P> {
    resources: Vec<ResourceDecl>,
    passes: Vec<PassDecl<P>>,
}

impl<P> Default for FrameGraph<P> {
    fn default() -> Self {
        FrameGraph {
            resources: Vec::new(),
            passes: Vec::new(),
        }
    }
}

/// Why a frame's declaration is invalid. Always a renderer bug.
#[derive(Debug, PartialEq, Eq)]
pub enum GraphError {
    /// A pass reads a transient texture before any pass writes it.
    ReadBeforeWrite {
        pass: &'static str,
        texture: &'static str,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::ReadBeforeWrite { pass, texture } => write!(
                f,
                "render graph: pass `{pass}` reads `{texture}` before any pass writes it"
            ),
        }
    }
}

impl<P> FrameGraph<P> {
    pub fn new() -> Self {
        Self::default()
    }

    /// A texture that lives only within this frame.
    pub fn create(&mut self, desc: TextureDesc) -> Res {
        self.push(Kind::Transient(desc))
    }

    /// A renderer-owned texture. Mark the frame's final image `output`.
    pub fn import(&mut self, name: &'static str, output: bool) -> Res {
        self.push(Kind::Imported { name, output })
    }

    fn push(&mut self, kind: Kind) -> Res {
        self.resources.push(ResourceDecl { kind });
        Res(self.resources.len() as u32 - 1)
    }

    /// Declare the next pass. Passes run in declaration order.
    pub fn add_pass(
        &mut self,
        pass: P,
        name: &'static str,
        reads: &[(Res, Access)],
        writes: &[(Res, Access)],
    ) {
        self.passes.push(PassDecl {
            pass,
            name,
            reads: reads.to_vec(),
            writes: writes.to_vec(),
        });
    }

    fn texture_name(&self, r: Res) -> &'static str {
        match &self.resources[r.0 as usize].kind {
            Kind::Transient(d) => d.label,
            Kind::Imported { name, .. } => name,
        }
    }

    /// Validate, cull, order and assign allocations.
    pub fn compile(self) -> Result<Plan<P>, GraphError> {
        // Every transient read needs an earlier writer.
        let mut written = vec![false; self.resources.len()];
        for p in &self.passes {
            for (r, _) in &p.reads {
                if matches!(self.resources[r.0 as usize].kind, Kind::Transient(_))
                    && !written[r.0 as usize]
                {
                    return Err(GraphError::ReadBeforeWrite {
                        pass: p.name,
                        texture: self.texture_name(*r),
                    });
                }
            }
            for (r, _) in &p.writes {
                written[r.0 as usize] = true;
            }
        }

        // Culling, back to front: a pass lives if it writes an output,
        // or writes something a later live pass reads.
        let mut live = vec![false; self.passes.len()];
        let mut needed = vec![false; self.resources.len()];
        for (i, p) in self.passes.iter().enumerate().rev() {
            let feeds = p.writes.iter().any(|(r, _)| {
                needed[r.0 as usize]
                    || matches!(
                        self.resources[r.0 as usize].kind,
                        Kind::Imported { output: true, .. }
                    )
            });
            if feeds {
                live[i] = true;
                for (r, _) in &p.reads {
                    needed[r.0 as usize] = true;
                }
            }
        }

        // Lifetimes (in live-pass steps) and usage of each transient.
        let n = self.resources.len();
        let mut first = vec![usize::MAX; n];
        let mut last = vec![0usize; n];
        let mut usage = vec![wgpu::TextureUsages::empty(); n];
        let mut step = 0;
        for (i, p) in self.passes.iter().enumerate() {
            if !live[i] {
                continue;
            }
            for (r, a) in p.reads.iter().chain(&p.writes) {
                let k = r.0 as usize;
                first[k] = first[k].min(step);
                last[k] = last[k].max(step);
                usage[k] |= a.usage();
            }
            step += 1;
        }

        // Greedy aliasing: reuse a slot whose previous tenant is done.
        let mut slots: Vec<Slot> = Vec::new();
        let mut slot_of = vec![None; n];
        let mut order: Vec<usize> = (0..n)
            .filter(|&k| first[k] != usize::MAX)
            .filter(|&k| matches!(self.resources[k].kind, Kind::Transient(_)))
            .collect();
        order.sort_by_key(|&k| first[k]);
        for k in order {
            let Kind::Transient(desc) = self.resources[k].kind else {
                continue;
            };
            let reuse = slots
                .iter()
                .position(|s| s.free_after < first[k] && s.desc.compatible(&desc));
            let s = match reuse {
                Some(s) => s,
                None => {
                    slots.push(Slot {
                        desc,
                        usage: wgpu::TextureUsages::empty(),
                        free_after: 0,
                    });
                    slots.len() - 1
                }
            };
            slots[s].usage |= usage[k];
            slots[s].free_after = last[k];
            slot_of[k] = Some(s);
        }

        let mut passes = Vec::new();
        for (i, p) in self.passes.into_iter().enumerate() {
            if live[i] {
                passes.push(p.pass);
            }
        }
        Ok(Plan {
            passes,
            slots,
            slot_of,
        })
    }
}

/// One physical transient texture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub desc: TextureDesc,
    /// Union of every access to any texture aliased onto this slot.
    pub usage: wgpu::TextureUsages,
    /// Last live step that uses the slot's current tenant.
    free_after: usize,
}

/// A compiled frame: the passes to run, in order, and the allocation
/// behind each transient texture.
pub struct Plan<P> {
    pub passes: Vec<P>,
    pub slots: Vec<Slot>,
    slot_of: Vec<Option<usize>>,
}

impl<P> Plan<P> {
    /// The slot backing transient `r`, or `None` if `r` is imported or
    /// no live pass touches it.
    pub fn slot(&self, r: Res) -> Option<usize> {
        self.slot_of.get(r.0 as usize).copied().flatten()
    }
}

/// GPU textures for a plan's slots, kept across frames.
#[derive(Default)]
pub struct TexturePool {
    entries: Vec<PoolEntry>,
}

struct PoolEntry {
    key: (TextureDesc, wgpu::TextureUsages, (u32, u32)),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Bumped whenever the texture is reallocated, so bind groups
    /// built on the old view know to rebuild.
    generation: u64,
}

static NEXT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl TexturePool {
    /// Make sure slot `i` of `plan` has a texture for a target of
    /// `width × height`, (re)allocating only when something changed.
    pub fn prepare<P>(&mut self, device: &wgpu::Device, plan: &Plan<P>, width: u32, height: u32) {
        for (i, slot) in plan.slots.iter().enumerate() {
            let size = slot.desc.extent.resolve(width, height);
            let key = (slot.desc, slot.usage, size);
            if self.entries.get(i).is_some_and(|e| e.key == key) {
                continue;
            }
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(slot.desc.label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: slot.desc.layers,
                },
                mip_level_count: slot.desc.mips,
                sample_count: slot.desc.samples,
                dimension: wgpu::TextureDimension::D2,
                format: slot.desc.format,
                usage: slot.usage,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let entry = PoolEntry {
                key,
                texture,
                view,
                generation: NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            };
            if i < self.entries.len() {
                self.entries[i] = entry;
            } else {
                self.entries.push(entry);
            }
        }
        self.entries.truncate(plan.slots.len());
    }

    /// The view behind transient `r` (after [`prepare`](Self::prepare)).
    pub fn view<P>(&self, plan: &Plan<P>, r: Res) -> Option<&wgpu::TextureView> {
        plan.slot(r).and_then(|s| self.entries.get(s)).map(|e| &e.view)
    }

    /// The texture behind transient `r`.
    pub fn texture<P>(&self, plan: &Plan<P>, r: Res) -> Option<&wgpu::Texture> {
        plan.slot(r).and_then(|s| self.entries.get(s)).map(|e| &e.texture)
    }

    /// Changes whenever `r`'s allocation is replaced.
    pub fn generation<P>(&self, plan: &Plan<P>, r: Res) -> u64 {
        plan.slot(r)
            .and_then(|s| self.entries.get(s))
            .map_or(0, |e| e.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::TextureFormat as F;

    fn hdr(label: &'static str) -> TextureDesc {
        TextureDesc::new(label, Extent::FULL, F::Rgba16Float)
    }

    #[test]
    fn passes_run_in_order_and_unused_ones_are_culled() {
        let mut g = FrameGraph::new();
        let target = g.import("target", true);
        let color = g.create(hdr("color"));
        let unused = g.create(hdr("debug view"));
        g.add_pass("main", "main", &[], &[(color, Access::Attach)]);
        g.add_pass("debug", "debug", &[(color, Access::Sample)], &[(unused, Access::Attach)]);
        g.add_pass("tonemap", "tonemap", &[(color, Access::Sample)], &[(target, Access::Attach)]);
        let plan = g.compile().unwrap();
        assert_eq!(plan.passes, ["main", "tonemap"]);
        assert_eq!(plan.slot(unused), None, "a culled texture gets no memory");
        assert_eq!(
            plan.slots[plan.slot(color).unwrap()].usage,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
        );
    }

    #[test]
    fn culling_follows_chains_back_from_the_output() {
        let mut g = FrameGraph::new();
        let target = g.import("target", true);
        let shadow = g.import("shadow map", false);
        let a = g.create(hdr("a"));
        g.add_pass(0, "shadow", &[], &[(shadow, Access::Attach)]);
        g.add_pass(1, "main", &[(shadow, Access::Sample)], &[(a, Access::Attach)]);
        g.add_pass(2, "out", &[(a, Access::Sample)], &[(target, Access::Attach)]);
        assert_eq!(g.compile().unwrap().passes, [0, 1, 2]);

        // Nothing reads the shadow map: its pass goes.
        let mut g = FrameGraph::new();
        let target = g.import("target", true);
        let shadow = g.import("shadow map", false);
        g.add_pass(0, "shadow", &[], &[(shadow, Access::Attach)]);
        g.add_pass(1, "clear", &[], &[(target, Access::Attach)]);
        assert_eq!(g.compile().unwrap().passes, [1]);
    }

    #[test]
    fn reading_an_unwritten_transient_is_an_error() {
        let mut g = FrameGraph::new();
        let target = g.import("target", true);
        let bloom = g.create(hdr("bloom"));
        g.add_pass((), "tonemap", &[(bloom, Access::Sample)], &[(target, Access::Attach)]);
        assert_eq!(
            g.compile().err(),
            Some(GraphError::ReadBeforeWrite {
                pass: "tonemap",
                texture: "bloom"
            })
        );
        // An imported texture may be read unwritten (it holds last frame).
        let mut g = FrameGraph::new();
        let target = g.import("target", true);
        let history = g.import("taa history", false);
        g.add_pass((), "resolve", &[(history, Access::Sample)], &[(target, Access::Attach)]);
        assert!(g.compile().is_ok());
    }

    #[test]
    fn textures_with_disjoint_lifetimes_share_memory() {
        // a → b → c → target: `a` is dead once `b` is written, so `c`
        // (same description) can take its allocation.
        let mut g = FrameGraph::new();
        let target = g.import("target", true);
        let a = g.create(hdr("a"));
        let b = g.create(hdr("b"));
        let c = g.create(hdr("c"));
        let depth = g.create(TextureDesc::new("depth", Extent::FULL, F::Depth32Float));
        g.add_pass(0, "p0", &[], &[(a, Access::Attach), (depth, Access::Attach)]);
        g.add_pass(1, "p1", &[(a, Access::Sample)], &[(b, Access::Attach)]);
        g.add_pass(2, "p2", &[(b, Access::Sample)], &[(c, Access::Attach)]);
        g.add_pass(3, "p3", &[(c, Access::Sample)], &[(target, Access::Attach)]);
        let plan = g.compile().unwrap();
        assert_eq!(plan.slot(a), plan.slot(c), "c reuses a's texture");
        assert_ne!(plan.slot(a), plan.slot(b), "b overlaps both");
        assert_ne!(plan.slot(depth), plan.slot(a), "different formats never alias");
        assert_eq!(plan.slots.len(), 3);
    }

    #[test]
    fn extents_scale_with_the_target() {
        assert_eq!(Extent::FULL.resolve(1280, 720), (1280, 720));
        assert_eq!(Extent::Target { shift: 1 }.resolve(1280, 720), (640, 360));
        assert_eq!(Extent::Target { shift: 12 }.resolve(1280, 720), (1, 1));
        assert_eq!(
            Extent::Fixed {
                width: 2048,
                height: 2048
            }
            .resolve(1, 1),
            (2048, 2048)
        );
    }
}
