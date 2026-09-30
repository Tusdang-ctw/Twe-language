use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::tagged_value::TaggedValue;

/// v0.2 Phase 8.5 session 8f: `Value` is a type alias for
/// `TaggedValue`. Every signature, struct field, and constructor
/// site in the codebase uses `Value` and gets the NaN-tagged
/// representation automatically. The legacy sum-type and the
/// `to_legacy` / `from_legacy` shim that bridged the migration
/// were deleted at the end of 8f.
pub type Value = TaggedValue;

/// web3d-M1: FxHash — the fast, non-cryptographic hash rustc uses
/// internally (the rustc-hash algorithm; ~15 lines, so no dependency).
/// Name lookups are the interpreter's hottest operation and their keys
/// are identifiers from the script, not attacker-chosen, so std's
/// DoS-resistant SipHash only costs time here.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const FX_SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl std::hash::Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            let w = u64::from_le_bytes(c.try_into().expect("8-byte chunk"));
            self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(FX_SEED);
        }
        for &b in chunks.remainder() {
            self.0 = (self.0.rotate_left(5) ^ u64::from(b)).wrapping_mul(FX_SEED);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.0 = (self.0.rotate_left(5) ^ u64::from(i)).wrapping_mul(FX_SEED);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// A `String`-keyed map using [`FxHasher`] — for env globals and
/// instance fields, the lookups on every variable access.
pub type NameMap<V> = HashMap<String, V, std::hash::BuildHasherDefault<FxHasher>>;

#[derive(Debug)]
pub struct FunctionDef {
    pub name: String,
    /// `Rc<str>` so binding a call's parameters is a refcount bump,
    /// not a string copy (web3d-M1).
    pub params: Vec<Rc<str>>,
    pub body: Vec<crate::ast::Stmt>,
    /// web3d-M1: the module object this function was defined in
    /// (`None` for the entry program). A call from another module
    /// resolves the body's free names in *this* module's globals —
    /// lexical scoping across modules.
    pub home: Option<TaggedValue>,
}

/// web3d-M3: an entity class's `look:` (`docs/06` §4.9a), with keys
/// merged along the `extends` chain — a subclass overrides single keys.
#[derive(Debug, Clone, Default)]
pub struct LookDef {
    pub mesh: Option<LookSlot>,
    pub tint: Option<LookSlot>,
    pub scale: Option<LookSlot>,
    pub facing: Option<LookSlot>,
    pub material: Option<LookSlot>,
}

/// One look key: its expression, where it was written, and whether it
/// reads the entity (so must be evaluated per entity) or is shared by
/// every entity of the class in a frame.
#[derive(Debug, Clone)]
pub struct LookSlot {
    pub expr: crate::ast::Expr,
    pub per_entity: bool,
    /// Defining module, as for [`MethodDef::home`].
    pub home: Option<TaggedValue>,
    pub line: u32,
    pub col: u32,
}

/// web3d-M3: an instance's fields, as a short vector — the class's
/// fields first, in [`ClassDef::field_layout`] order (so every instance
/// of a class keeps a field at the same position, which
/// `ast::ResCell` hints cache), then any added at run time. Instances
/// have a handful of fields, where a scan beats hashing.
#[derive(Debug, Clone, Default)]
pub struct Fields(Vec<(Rc<str>, TaggedValue)>);

impl Fields {
    pub fn from_layout(layout: &[(Rc<str>, TaggedValue)]) -> Self {
        Fields(layout.to_vec())
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.0.iter().position(|(n, _)| **n == *name)
    }

    pub fn get(&self, name: &str) -> Option<&TaggedValue> {
        self.0.iter().find(|(n, _)| **n == *name).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut TaggedValue> {
        self.0
            .iter_mut()
            .find(|(n, _)| **n == *name)
            .map(|(_, v)| v)
    }

    pub fn contains_key(&self, name: &str) -> bool {
        self.index_of(name).is_some()
    }

    /// Set a field, adding it if absent.
    pub fn insert(&mut self, name: impl AsRef<str>, value: TaggedValue) {
        let name = name.as_ref();
        match self.get_mut(name) {
            Some(slot) => *slot = value,
            None => self.0.push((Rc::from(name), value)),
        }
    }

    /// The field at position `i`, if it is `name` (a stale hint reads
    /// as `None`).
    #[inline]
    pub fn at(&self, i: u32, name: &str) -> Option<TaggedValue> {
        match self.0.get(i as usize) {
            Some((n, v)) if **n == *name => Some(*v),
            _ => None,
        }
    }

    /// Write the field at position `i` if it is `name`.
    #[inline]
    pub fn set_at(&mut self, i: u32, name: &str, value: TaggedValue) -> bool {
        match self.0.get_mut(i as usize) {
            Some((n, v)) if **n == *name => {
                *v = value;
                true
            }
            _ => false,
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(n, _)| &**n)
    }

    pub fn values(&self) -> impl Iterator<Item = &TaggedValue> {
        self.0.iter().map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &TaggedValue)> {
        self.0.iter().map(|(n, v)| (&**n, v))
    }
}

#[derive(Debug)]
pub struct ClassDef {
    pub kind: &'static str,
    pub name: String,
    pub parent: Option<Rc<ClassDef>>,
    pub field_defaults: HashMap<String, TaggedValue>,
    pub methods: NameMap<Rc<MethodDef>>,
    pub states: HashMap<String, Rc<StateDef>>,
    pub initial_state: Option<String>,
    /// web3d-M3: the merged `look:`, if this class or an ancestor has one.
    pub look: Option<Rc<LookDef>>,
    /// web3d-M3: every field an instance starts with — ancestors'
    /// first, a subclass's defaults overriding — in a fixed order, so
    /// instances share field positions. Built once per class.
    pub field_layout: Vec<(Rc<str>, TaggedValue)>,
}

#[derive(Debug)]
pub struct MethodDef {
    pub params: Vec<Rc<str>>,
    pub body: Vec<crate::ast::Stmt>,
    /// web3d-M1: defining module, as for [`FunctionDef::home`].
    pub home: Option<TaggedValue>,
}

/// web3d-M1: one activation of a function, method, or event-handler
/// body — its local variables (parameters first) plus the module whose
/// globals its free names resolve in. Block visibility is checked
/// statically by `crate::resolve`, so a frame is one flat list; it is
/// a `Vec` because frames are small and linear lookup beats hashing.
#[derive(Debug, Clone, Default)]
pub struct LocalFrame {
    pub locals: Vec<(Rc<str>, TaggedValue)>,
    /// Module object whose fields are this frame's globals, when the
    /// body was defined in a module other than the running one.
    pub home: Option<TaggedValue>,
}

impl LocalFrame {
    pub fn get(&self, name: &str) -> Option<TaggedValue> {
        self.locals
            .iter()
            .rev()
            .find(|(n, _)| &**n == name)
            .map(|(_, v)| *v)
    }

    /// Set an existing local; returns false if `name` isn't one.
    pub fn assign(&mut self, name: &str, v: TaggedValue) -> bool {
        match self.locals.iter_mut().rev().find(|(n, _)| &**n == name) {
            Some(slot) => {
                slot.1 = v;
                true
            }
            None => false,
        }
    }

    /// Remove the innermost local named `name`, if any.
    pub fn remove(&mut self, name: &str) {
        if let Some(i) = self.locals.iter().rposition(|(n, _)| &**n == name) {
            self.locals.remove(i);
        }
    }

    /// Declare (or re-declare) a local.
    pub fn declare(&mut self, name: &str, v: TaggedValue) {
        if !self.assign(name, v) {
            self.locals.push((Rc::from(name), v));
        }
    }

    fn mark(&self) {
        for (_, v) in &self.locals {
            crate::heap::mark_value(v);
        }
        if let Some(h) = &self.home {
            crate::heap::mark_value(h);
        }
    }
}

#[derive(Debug)]
pub struct StateDef {
    pub name: String,
    pub on_entry: Vec<crate::ast::Stmt>,
    pub every_clocks: Vec<EveryClockDef>,
    pub on_render: Option<Vec<crate::ast::Stmt>>,
    pub on_key_press: HashMap<String, Vec<crate::ast::Stmt>>,
    /// State-scoped `on update(dt):`. Fires once per frame with the
    /// real dt while this state is active. Closes Phase 2 F5.
    pub on_update: Option<OnUpdateHandler>,
    /// State-scoped `on <predicate>:` handlers. Each entry is the
    /// predicate expression and its body. The runtime tracks each
    /// predicate's last evaluated truthiness on the active
    /// instance and fires the body on a false → true transition
    /// (edge-triggered). Phase 5 task 4 (Example 4 surface).
    pub on_predicates: Vec<PredicateHandlerDef>,
    /// `on exit:` body — runs when the state is left, just before the
    /// next state's entry. `None` if the state declares no exit hook.
    pub on_exit: Option<Vec<crate::ast::Stmt>>,
}

#[derive(Debug)]
pub struct PredicateHandlerDef {
    pub predicate: crate::ast::Expr,
    pub body: Vec<crate::ast::Stmt>,
}

#[derive(Debug)]
pub struct EveryClockDef {
    pub interval: crate::ast::Expr,
    pub body: Vec<crate::ast::Stmt>,
}

#[derive(Debug)]
pub struct Instance {
    pub class: Rc<ClassDef>,
    /// web3d-M3: see [`Fields`].
    pub fields: Fields,
    pub current_state: Option<String>,
    /// Accumulated seconds since each clock last fired, parallel-indexed
    /// to `current_state`'s `every_clocks`.
    pub every_timers: Vec<f64>,
    /// Cached interval seconds for each clock in `current_state`.
    pub every_intervals_secs: Vec<f64>,
    /// Set by `despawn self`; the runtime drops this instance from
    /// `Env::active_entities` at the end of the frame.
    pub despawned: bool,
    /// True after the death-event handler (registered via
    /// `on <Class>.death(e):`) has fired for this instance. Avoids
    /// re-firing if the entity stays in `active_entities` for
    /// multiple frames before pruning. Phase 9 session 7b.
    pub death_fired: bool,
    /// Suspended-fiber call stack. Empty = not suspended (entry
    /// ran to completion, or the state has no entry body / hasn't
    /// been entered). Non-empty = a `wait` fired somewhere in the
    /// state's `on_entry` body (possibly inside nested `if` /
    /// `while` blocks, or inside a function called from there).
    ///
    /// Bottom frame (`fiber_frames[0]`) is always
    /// `FrameKind::StateEntry`. Frames above are function calls
    /// whose body suspended. Each frame carries its own
    /// `resume_path`. On resume, the runner pops frames top-down:
    /// the innermost completes first, then its parent, until the
    /// state-entry's path runs to completion. v0.2 sessions 2a
    /// (path) + 2b (frame stack).
    pub fiber_frames: Vec<Frame>,
    /// Seconds left on the active `wait`. Decremented by `dt` each
    /// frame the instance ticks.
    pub entry_wait_remaining: f64,
    /// Phase 5 task 4: parallel-indexed to the active state's
    /// `on_predicates`. Records the last evaluated truthiness of
    /// each predicate so the runtime can detect false → true
    /// transitions (edge-triggered firing). Reset on state entry.
    pub predicate_last_values: Vec<bool>,
    /// web3d-M1: this instance's GC wrapper, created once and reused so
    /// per-frame calls (`update`, scene handlers) don't allocate a new
    /// heap object each time. A raw (non-owning) pointer: the GC traces
    /// it only through the instance (`mark_instance` / `mark_body`), so
    /// once nothing reaches the instance the wrapper is swept, and its
    /// `Rc` drop frees the instance — no cycle leak.
    pub cached_value: Option<TaggedValue>,
}

impl Instance {
    /// Read a field. v0.2 Phase 8.5 session 8f: returns the
    /// stored `TaggedValue` (Value alias) directly.
    pub fn get_field(&self, name: &str) -> Option<TaggedValue> {
        self.fields.get(name).cloned()
    }

    pub fn insert_field(&mut self, name: impl AsRef<str>, value: TaggedValue) {
        self.fields.insert(name, value);
    }

    /// web3d-M1: overwrite an existing field in place — no key
    /// allocation, unlike `insert_field`. Returns false if absent.
    pub fn set_existing_field(&mut self, name: &str, value: TaggedValue) -> bool {
        match self.fields.get_mut(name) {
            Some(slot) => {
                *slot = value;
                true
            }
            None => false,
        }
    }
}

/// One step on a fiber's resume path. Each step describes one
/// nesting depth of the suspended body the frame is rooted in.
/// v0.2 session 2a.
///
/// All steps except the last have `branch == Some(...)` indicating
/// which sub-body of `body[stmt_index]` was descended into. The
/// last step has `branch == None` and `stmt_index` is the index
/// of the next statement to execute in the body at that depth.
#[derive(Debug, Clone)]
pub struct PathEntry {
    pub stmt_index: usize,
    pub branch: Option<Branch>,
}

/// Which sub-body of a structured statement was descended into.
/// `IfElif(idx)` records which arm of an `if`'s elif chain matched
/// at suspension time so resume picks the same arm without
/// re-evaluating its condition (preserves side-effect order). v0.2
/// session 2a covers `if` / `elif` / `else` and `while` bodies;
/// `for` bodies still surface the original wait-context error
/// (deferred to a follow-on session — needs iterator-state
/// preservation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Branch {
    IfThen,
    IfElif(usize),
    IfElse,
    While,
    /// Resuming the body of a `<action> then <body>` after the action's
    /// wait has elapsed.
    Then,
}

/// One frame on a suspended fiber's call stack. v0.2 session 2b.
///
/// The bottom frame is always `FrameKind::StateEntry` — the
/// state's `on_entry` body. Frames above it are function calls
/// whose body suspended on a `wait` (or whose body called another
/// function that suspended). Each frame carries its own
/// `resume_path` describing where in its own body to resume.
#[derive(Debug, Clone)]
pub struct Frame {
    pub kind: FrameKind,
    pub resume_path: Vec<PathEntry>,
    /// web3d-M1: the body's local variables, parked here while the
    /// fiber is suspended and pushed back onto `Env::frames` on resume.
    pub locals: LocalFrame,
}

/// What body the frame is rooted in. `StateEntry` is fetched at
/// resume from the instance's current state (so hot-reloads
/// recompute the body). `Function` holds an `Rc<FunctionDef>` so
/// the body stays resolvable even if the function gets
/// redefined / removed after the suspension. The function
/// variant also carries the bookkeeping needed to restore env
/// state when the frame eventually completes:
///
/// - `saved_returning`: the value of `env.returning` at the time
///   of the call (almost always `None`). Restored on completion
///   so a parent function's return channel isn't corrupted.
///
/// (web3d-M1 removed `saved_params`: parameters are frame locals now,
/// saved with the rest of the frame in `Frame::locals`.)
///
/// v0.2 session 2b targets function-position `Stmt::Expr` calls
/// only; method dispatch and call-as-expression (`let x = f()`)
/// are follow-ons.
#[derive(Debug, Clone)]
pub enum FrameKind {
    StateEntry,
    Function {
        def: Rc<crate::value::FunctionDef>,
        saved_returning: Option<TaggedValue>,
    },
}

#[derive(Debug, Default)]
pub struct Object {
    /// v0.2 Phase 8.5 session 8e: stored as `TaggedValue`. Every
    /// stdlib module (`math`, `key`, `screen`, `time`, `color`,
    /// `sprite`, `entities`, etc.) is an `Object`; the field
    /// storage migrates here so the GC roots (8h) can scan
    /// stdlib state via the same path as user-defined values.
    pub fields: HashMap<String, TaggedValue>,
    pub kind: &'static str,
}

impl Object {
    /// Read a field. v0.2 Phase 8.5 session 8f: returns the
    /// `TaggedValue` directly.
    pub fn get_field(&self, name: &str) -> Option<TaggedValue> {
        self.fields.get(name).cloned()
    }

    pub fn insert_field(&mut self, name: impl Into<String>, value: TaggedValue) {
        self.fields.insert(name.into(), value);
    }
}

pub type BuiltinFn = fn(&mut Env, &[TaggedValue]) -> Result<TaggedValue, RuntimeError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub help: Option<String>,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)?;
        if let Some(help) = &self.help {
            write!(f, "\n  help: {help}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RuntimeError {}

pub struct Env {
    /// Globals. web3d-M3: stored by index — `global_names[i]` /
    /// `global_values[i]`, with `global_index` mapping a name to its
    /// index — so a name's index can be cached (`ast::ResCell` hint)
    /// and read back without hashing. Indices never move: removing a
    /// global leaves `None` in its slot.
    global_names: Vec<Rc<str>>,
    global_values: Vec<Option<TaggedValue>>,
    global_index: NameMap<u32>,
    pub out: String,
    pub on_update: Option<OnUpdateHandler>,
    /// Top-level `on render():` handler — runs once per rendered
    /// frame in `twec play3d`. State-scoped on_render lives on
    /// `StateDef` and is for the 2D macroquad path.
    pub top_on_render: Option<Vec<crate::ast::Stmt>>,
    /// Class-name → registered death handlers. Phase 9 session 7b.
    /// Multiple handlers per class fire in registration order; the
    /// dying entity is bound to the handler's `param` for the body's
    /// scope. Only the tree-walker reads this in v0.3 — the bytecode
    /// VM mirror is a follow-on.
    pub death_handlers: HashMap<String, Vec<OnDeathHandler>>,
    pub active_scene: Option<Rc<RefCell<Instance>>>,
    pub active_entities: Vec<Rc<RefCell<Instance>>>,
    pub self_value: Option<TaggedValue>,
    pub returning: Option<TaggedValue>,
    pub transitioning: Option<String>,
    pub breaking: bool,
    pub continuing: bool,
    pub in_render: bool,
    pub loop_depth: u32,
    pub call_depth: u32,
    /// web3d-M1: active function / method / handler frames, innermost
    /// last. Empty while running top-level statements.
    pub frames: Vec<LocalFrame>,
    /// web3d-M1: cleared locals vectors from finished frames, reused by
    /// the next call so a call doesn't allocate its frame.
    pub frame_pool: Vec<Vec<(Rc<str>, TaggedValue)>>,
    /// web3d-M3: spare argument vectors for calls, so evaluating a
    /// call's arguments doesn't allocate (`eval::eval_args`).
    pub arg_pool: Vec<Vec<TaggedValue>>,
    /// web3d-M3: simulation time in seconds — the sum of every tick's
    /// `dt`. Deterministic (replays see the same value); materials
    /// animate on it.
    pub sim_time: f64,
    /// web3d-M3: each `visual` block compiled as a mesh material
    /// (`visual_wgsl::compile_material`), by name; `Err` holds why it
    /// can't be one.
    pub visual_materials: HashMap<String, Result<String, String>>,
    /// Material id → WGSL, as the kernel's snapshot takes it; index 0
    /// is the plain surface. See [`Env::intern_material`].
    pub material_sources: Vec<String>,
    /// web3d-M3: this frame's HUD (`text()` / `rect()` in a 3D render).
    pub hud_queue: Vec<crate::render3d_types::HudItem>,
    material_names: Vec<String>,
    /// web3d-M7: whether `particles` blocks that compile to WGSL run on
    /// the GPU (`kernel::particles`). The 3D hosts set it; elsewhere
    /// every particle runs on the CPU.
    pub gpu_particles: bool,
    /// web3d-M7: each `particles` block compiled for the GPU, by name;
    /// `Err` holds why it stays on the CPU.
    pub particle_classes: HashMap<String, Result<crate::kernel::particles::ParticleProgram, String>>,
    /// Particle program id → program, as the kernel's snapshot takes it.
    pub particle_programs: Vec<crate::kernel::particles::ParticleProgram>,
    particle_program_names: Vec<String>,
    /// GPU emissions since the host last drained them.
    pub particle_emissions: Vec<crate::kernel::particles::ParticleEmission>,
    /// Counts emitters, seeding each one's random stream.
    pub particle_seed: u32,
    /// web3d-M1: the module object being initialised when this env runs
    /// a module's top level (`None` for the entry program). A function
    /// whose `home` is this module resolves globals in this env.
    pub current_module: Option<TaggedValue>,
    /// 3D draw queue accumulated across one frame's `on render():`
    /// body. `cube(at:, color:, size:)` and friends push here; the
    /// `play3d` render loop drains and consumes after the body
    /// finishes. Cleared at the start of each frame.
    pub render_queue3d: Vec<DrawCall3d>,
    /// Path-interning registry for `Primitive::Mesh(id)`. Indices
    /// are stable across frames (and across hot-reloads, as long as
    /// the new env intern-orders match — typically yes since
    /// `mesh()` calls run in source order). v0.2 session 1.
    pub mesh_paths: Vec<String>,
    /// Phase 17 session 3: path-interning registry for textures
    /// referenced via `texture("foo.png")`. The handle returned by
    /// `texture()` carries the interned id; the play3d loop reads
    /// `DrawCall3d::texture` and uploads/binds the matching PNG.
    /// Id 0 is reserved for "no texture" (white fallback).
    pub texture_paths: Vec<String>,
    rng_state: u64,
    /// Phase 13 session 3: cache of evaluated modules keyed by their
    /// canonical filesystem path (as a string for hashability). The
    /// value is the module-value Object whose fields are the
    /// module's top-level bindings — so `import "math"` followed by
    /// `math.add(1, 2)` is just two ordinary lookups against this
    /// cache + an Object field-get.
    pub module_cache: HashMap<String, TaggedValue>,
    /// Path of the source file currently being evaluated. Used by
    /// `Stmt::Import` to resolve relative module paths the same way
    /// the loader (`module::resolve`) does. Populated by the entry
    /// runner; defaults to `None` for ad-hoc `eval::run` callers.
    pub current_source: Option<std::path::PathBuf>,
}

// web3d-M2: `DrawCall3d` / `Primitive` are renderer-kernel data; they
// live in `render3d_types` and are re-exported here for existing paths.
pub use crate::render3d_types::{DrawCall3d, Primitive};

#[derive(Clone, Debug)]
pub struct OnUpdateHandler {
    pub param: String,
    pub body: Vec<crate::ast::Stmt>,
}

/// Handler registered via `on <Class>.death(e):`. Multiple handlers
/// for the same class are allowed and fire in registration order.
/// Phase 9 session 7b.
#[derive(Clone, Debug)]
pub struct OnDeathHandler {
    pub param: String,
    pub body: Vec<crate::ast::Stmt>,
}

impl Env {
    pub fn new() -> Self {
        Self {
            global_names: Vec::new(),
            global_values: Vec::new(),
            global_index: NameMap::default(),
            out: String::new(),
            on_update: None,
            top_on_render: None,
            death_handlers: HashMap::new(),
            active_scene: None,
            active_entities: Vec::new(),
            self_value: None,
            returning: None,
            transitioning: None,
            breaking: false,
            continuing: false,
            in_render: false,
            loop_depth: 0,
            call_depth: 0,
            frames: Vec::new(),
            frame_pool: Vec::new(),
            arg_pool: Vec::new(),
            sim_time: 0.0,
            visual_materials: HashMap::new(),
            material_sources: vec![String::new()],
            hud_queue: Vec::new(),
            material_names: vec![String::new()],
            gpu_particles: false,
            particle_classes: HashMap::new(),
            particle_programs: Vec::new(),
            particle_program_names: Vec::new(),
            particle_emissions: Vec::new(),
            particle_seed: 0,
            current_module: None,
            render_queue3d: Vec::new(),
            mesh_paths: Vec::new(),
            texture_paths: Vec::new(),
            // xorshift64* seeded from a fixed constant for deterministic
            // tests. CLI can override via `twec run --seed N`.
            rng_state: 0x9E37_79B9_7F4A_7C15,
            module_cache: HashMap::new(),
            current_source: None,
        }
    }

    /// Find-or-insert `path` in the mesh-path registry. Returns the
    /// interned id, used as the payload of `Primitive::Mesh`. Linear
    /// scan is fine — a Twe scene rarely uses more than a handful of
    /// distinct meshes, and this only runs in `mesh()` calls inside
    /// `on render():`.
    /// web3d-M3: the material id for visual `name`, compiling it into
    /// the snapshot's table on first use.
    pub fn intern_material(&mut self, name: &str) -> Result<u32, String> {
        if let Some(i) = self.material_names.iter().position(|n| n == name) {
            return Ok(i as u32);
        }
        let wgsl = match self.visual_materials.get(name) {
            Some(Ok(w)) => w.clone(),
            Some(Err(e)) => return Err(e.clone()),
            None => return Err(format!("`{name}` is not a visual block")),
        };
        self.material_names.push(name.to_string());
        self.material_sources.push(wgsl);
        Ok((self.material_names.len() - 1) as u32)
    }

    pub fn intern_mesh_path(&mut self, path: &str) -> u32 {
        if let Some(idx) = self.mesh_paths.iter().position(|p| p == path) {
            return idx as u32;
        }
        let idx = self.mesh_paths.len() as u32;
        self.mesh_paths.push(path.to_string());
        idx
    }

    /// Reverse lookup for the render side. Returns `None` for an id
    /// that was never interned in this env (only happens after a
    /// hot-reload that drops a `mesh()` call).
    pub fn mesh_path(&self, id: u32) -> Option<&str> {
        self.mesh_paths.get(id as usize).map(String::as_str)
    }

    /// Phase 17 session 3: find-or-insert a texture path. Returns a
    /// 1-based interned id (0 is reserved for "no texture / white
    /// fallback"). Used by the `texture()` builtin and threaded
    /// through `DrawCall3d::texture` to the play3d loop.
    pub fn intern_texture_path(&mut self, path: &str) -> u32 {
        if let Some(idx) = self.texture_paths.iter().position(|p| p == path) {
            return (idx + 1) as u32;
        }
        let idx = self.texture_paths.len() as u32 + 1;
        self.texture_paths.push(path.to_string());
        idx
    }

    /// Reverse lookup for textures. `id == 0` returns None (white
    /// fallback). `id >= 1` indexes into `texture_paths` (offset by
    /// 1 because 0 is reserved).
    pub fn texture_path(&self, id: u32) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.texture_paths
            .get((id - 1) as usize)
            .map(String::as_str)
    }

    /// xorshift64* PRNG. Deterministic given a fixed seed.
    pub fn next_random_u64(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// web3d-M7: swap in another random stream, returning the current
    /// one. Particle bodies draw from their emitter's own stream, so
    /// spawning particles never changes the script's random numbers.
    pub fn swap_rng(&mut self, state: u64) -> u64 {
        std::mem::replace(&mut self.rng_state, if state == 0 { 0x9E37_79B9_7F4A_7C15 } else { state })
    }

    /// web3d-M7: the GPU program id for particles block `name`, if it
    /// compiled, interning it on first use.
    pub fn intern_particle_program(&mut self, name: &str) -> Option<u32> {
        if let Some(i) = self.particle_program_names.iter().position(|n| n == name) {
            return Some(i as u32);
        }
        let program = self.particle_classes.get(name)?.as_ref().ok()?.clone();
        self.particle_program_names.push(name.to_string());
        self.particle_programs.push(program);
        Some((self.particle_programs.len() - 1) as u32)
    }

    pub fn seed_rng(&mut self, seed: u64) {
        // xorshift cannot be seeded with zero; substitute a non-zero value.
        self.rng_state = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
    }

    /// Look up a binding. v0.2 Phase 8.5 session 8f: returns the
    /// stored `TaggedValue` directly.
    pub fn get(&self, name: &str) -> Option<TaggedValue> {
        let i = *self.global_index.get(name)?;
        self.global_values[i as usize]
    }

    pub fn set(&mut self, name: String, value: TaggedValue) {
        match self.global_index.get(name.as_str()) {
            Some(&i) => self.global_values[i as usize] = Some(value),
            None => {
                let i = u32::try_from(self.global_values.len()).expect("fewer than 2^32 globals");
                self.global_names.push(Rc::from(name.as_str()));
                self.global_values.push(Some(value));
                self.global_index.insert(name, i);
            }
        }
    }

    /// web3d-M1: update an existing global in place (no key
    /// allocation); returns false if `name` isn't bound.
    pub fn assign_existing(&mut self, name: &str, value: TaggedValue) -> bool {
        match self.global_index.get(name) {
            Some(&i) => match &mut self.global_values[i as usize] {
                Some(slot) => {
                    *slot = value;
                    true
                }
                None => false,
            },
            None => false,
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn remove(&mut self, name: &str) {
        if let Some(&i) = self.global_index.get(name) {
            self.global_values[i as usize] = None;
        }
    }

    /// web3d-M3: the index of global `name`, for caching.
    pub fn global_slot(&self, name: &str) -> Option<u32> {
        self.global_index.get(name).copied()
    }

    /// web3d-M3: the global at a cached index, if that index still
    /// holds `name` (a stale or foreign hint reads as `None`).
    #[inline]
    pub fn global_at(&self, i: u32, name: &str) -> Option<TaggedValue> {
        let i = i as usize;
        match self.global_names.get(i) {
            Some(n) if **n == *name => self.global_values[i],
            _ => None,
        }
    }

    /// web3d-M3: [`Env::global_at`] for writing; false when the hint
    /// doesn't hold `name` or the global was removed.
    #[inline]
    pub fn assign_global_at(&mut self, i: u32, name: &str, value: TaggedValue) -> bool {
        let i = i as usize;
        match (self.global_names.get(i), self.global_values.get_mut(i)) {
            (Some(n), Some(Some(slot))) if **n == *name => {
                *slot = value;
                true
            }
            _ => false,
        }
    }

    /// Iterate over every (name, value) currently bound. v0.2 Phase
    /// 8.5 session 8f: yields owned `(String, TaggedValue)` tuples.
    pub fn iter_bindings(&self) -> impl Iterator<Item = (String, TaggedValue)> + '_ {
        self.global_names
            .iter()
            .zip(&self.global_values)
            .filter_map(|(n, v)| v.map(|v| (n.to_string(), v)))
    }

    /// v0.2 Phase 8.5 session 8h: walk every GC root reachable through
    /// this env and mark them. Called from a safepoint inside
    /// `gc_collect_with(|| env.scan_roots())`.
    ///
    /// Roots: bindings (every global), self_value, returning, the
    /// active scene's instance fields + fiber frames, and every
    /// active entity's instance fields + fiber frames.
    pub fn scan_roots(&self) {
        for v in self.global_values.iter().flatten() {
            crate::heap::mark_value(v);
        }
        if let Some(v) = &self.self_value {
            crate::heap::mark_value(v);
        }
        if let Some(v) = &self.returning {
            crate::heap::mark_value(v);
        }
        if let Some(scene) = &self.active_scene {
            mark_instance(&scene.borrow());
        }
        for ent in &self.active_entities {
            mark_instance(&ent.borrow());
        }
        // web3d-M0: imported modules are reachable only through the
        // cache until an `import` binds them; stdlib thread-locals
        // (save store, plural-rule closures) hold script values too.
        for v in self.module_cache.values() {
            crate::heap::mark_value(v);
        }
        // web3d-M1: locals of every active function / handler frame.
        for f in &self.frames {
            f.mark();
        }
        if let Some(m) = &self.current_module {
            crate::heap::mark_value(m);
        }
        crate::stdlib::scan_stdlib_roots();
    }
}

/// Mark every `TaggedValue` reachable through an `Instance` — its
/// fields plus any saved fiber-frame state. Used by `Env::scan_roots`
/// (where active_scene/active_entities hold naked `Rc<RefCell<Instance>>`
/// without a corresponding TaggedValue), and any other site that
/// roots an instance directly. v0.2 Phase 8.5 session 8h.
pub fn mark_instance(inst: &Instance) {
    for v in inst.fields.values() {
        crate::heap::mark_value(v);
    }
    if let Some(v) = &inst.cached_value {
        crate::heap::mark_value(v);
    }
    for frame in &inst.fiber_frames {
        mark_fiber_frame(frame);
    }
}

/// Mark the values a suspended fiber frame keeps alive: its parked
/// locals (web3d-M1) and, for a function frame, the saved return slot.
pub fn mark_fiber_frame(frame: &Frame) {
    frame.locals.mark();
    if let FrameKind::Function {
        saved_returning: Some(v),
        ..
    } = &frame.kind
    {
        crate::heap::mark_value(v);
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

/// Find the closest match in `candidates` for the misspelled name
/// `target`, using Damerau-style edit distance ≤ 2. Returns
/// `Some(name)` only when a clear single best candidate exists —
/// no result for empty candidate sets, names that already match
/// exactly, or ties that would be unhelpful to print. Phase 6
/// session 4 (error-message polish).
///
/// Bounded distance: 1 for short names (≤ 4 chars), 2 for longer
/// names. Stops users seeing "did you mean: foo?" when they typed
/// something that bears no relation to any known name. The cost
/// of a false suggestion (confused user pursuing a wrong fix) is
/// higher than the cost of no suggestion.
pub fn did_you_mean<'a, I, S>(target: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a S>,
    S: AsRef<str> + 'a + ?Sized,
{
    if target.is_empty() {
        return None;
    }
    let limit = if target.chars().count() <= 4 { 1 } else { 2 };
    let mut best: Option<(&str, usize)> = None;
    for c in candidates {
        let cand = c.as_ref();
        if cand == target {
            // Exact match — no suggestion to make.
            return None;
        }
        let d = edit_distance(target, cand, limit + 1);
        if d <= limit {
            match best {
                None => best = Some((cand, d)),
                Some((_, bd)) if d < bd => best = Some((cand, d)),
                Some((_, bd)) if d == bd => {
                    // Tie: don't print either. Two equally close
                    // matches usually means the user had a
                    // different name in mind entirely.
                    best = None;
                }
                _ => {}
            }
        }
    }
    best.map(|(name, _)| name)
}

/// Bounded Levenshtein distance — returns the actual distance up
/// to `cap`, or any value > `cap` when the strings are further
/// apart than that. The cap turns the inner loop into early-exit
/// once a row's minimum exceeds the cap, which keeps `did_you_mean`
/// fast on long candidate lists.
fn edit_distance(a: &str, b: &str, cap: usize) -> usize {
    let a_bytes: Vec<char> = a.chars().collect();
    let b_bytes: Vec<char> = b.chars().collect();
    let n = a_bytes.len();
    let m = b_bytes.len();
    if n.abs_diff(m) > cap {
        return cap + 1;
    }
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr: Vec<usize> = vec![0; m + 1];
    for i in 1..=n {
        curr[0] = i;
        let mut row_min = curr[0];
        for j in 1..=m {
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            if curr[j] < row_min {
                row_min = curr[j];
            }
        }
        if row_min > cap {
            return cap + 1;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
}

#[cfg(test)]
mod did_you_mean_tests {
    use super::*;

    #[test]
    fn finds_one_char_typo() {
        let candidates = vec!["math.abs".to_string(), "math.cos".to_string()];
        assert_eq!(did_you_mean("math.cs", &candidates), Some("math.cos"));
    }

    #[test]
    fn returns_none_for_unrelated() {
        let candidates = vec!["math.abs".to_string()];
        assert_eq!(did_you_mean("xyzzy", &candidates), None);
    }

    #[test]
    fn returns_none_on_exact_match() {
        let candidates = vec!["foo".to_string()];
        assert_eq!(did_you_mean("foo", &candidates), None);
    }

    #[test]
    fn returns_none_on_tie() {
        // Two equally close — don't pick either.
        let candidates = vec!["abc".to_string(), "abd".to_string()];
        assert_eq!(did_you_mean("abe", &candidates), None);
    }

    #[test]
    fn short_names_use_distance_1() {
        // "ax" vs "by" is distance 2 — too far for a 2-char target.
        let candidates = vec!["by".to_string()];
        assert_eq!(did_you_mean("ax", &candidates), None);
        // Distance 1 is fine.
        let candidates = vec!["ay".to_string()];
        assert_eq!(did_you_mean("ax", &candidates), Some("ay"));
    }

    #[test]
    fn longer_names_use_distance_2() {
        // "function" vs "funciton" — two char swaps from each other.
        let candidates = vec!["function".to_string()];
        assert_eq!(did_you_mean("funciton", &candidates), Some("function"));
    }
}

#[cfg(test)]
mod mesh_registry_tests {
    use super::*;

    #[test]
    fn intern_mesh_path_is_stable() {
        let mut env = Env::new();
        let id_a = env.intern_mesh_path("a.glb");
        let id_b = env.intern_mesh_path("b.glb");
        assert_ne!(id_a, id_b);
        // Re-interning returns the same id.
        assert_eq!(env.intern_mesh_path("a.glb"), id_a);
        assert_eq!(env.intern_mesh_path("b.glb"), id_b);
    }

    #[test]
    fn mesh_path_round_trip() {
        let mut env = Env::new();
        let id = env.intern_mesh_path("models/box.glb");
        assert_eq!(env.mesh_path(id), Some("models/box.glb"));
    }

    #[test]
    fn mesh_path_out_of_range_returns_none() {
        let env = Env::new();
        assert_eq!(env.mesh_path(0), None);
        assert_eq!(env.mesh_path(99), None);
    }
}
