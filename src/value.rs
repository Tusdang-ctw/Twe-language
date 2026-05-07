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

#[derive(Debug)]
pub struct FunctionDef {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<crate::ast::Stmt>,
}

#[derive(Debug)]
pub struct ClassDef {
    pub kind: &'static str,
    pub name: String,
    pub parent: Option<Rc<ClassDef>>,
    pub field_defaults: HashMap<String, TaggedValue>,
    pub methods: HashMap<String, Rc<MethodDef>>,
    pub states: HashMap<String, Rc<StateDef>>,
    pub initial_state: Option<String>,
}

#[derive(Debug)]
pub struct MethodDef {
    pub params: Vec<String>,
    pub body: Vec<crate::ast::Stmt>,
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
    /// v0.2 Phase 8.5 session 8d: instance fields stored as
    /// `TaggedValue`. Direct accessors in `eval` shim with
    /// `to_legacy()` / `from_legacy()` at the boundary; the
    /// interior pattern matches still operate on legacy `Value`
    /// until 8f.
    pub fields: HashMap<String, TaggedValue>,
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
}

impl Instance {
    /// Read a field. v0.2 Phase 8.5 session 8f: returns the
    /// stored `TaggedValue` (Value alias) directly.
    pub fn get_field(&self, name: &str) -> Option<TaggedValue> {
        self.fields.get(name).cloned()
    }

    pub fn insert_field(&mut self, name: impl Into<String>, value: TaggedValue) {
        self.fields.insert(name.into(), value);
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
/// - `saved_params`: the previous bindings the function's
///   parameter names shadowed. Restored on completion.
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
        saved_params: Vec<(String, Option<TaggedValue>)>,
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
    /// v0.2 Phase 8.5 session 8d: globals stored as
    /// `TaggedValue`. The `get` / `set` / `iter_bindings` API
    /// converts at the boundary so external callers (eval,
    /// stdlib) keep working on legacy `Value` until 8e/8f.
    bindings: HashMap<String, TaggedValue>,
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

/// One queued 3D primitive. Phase 6 session 7 added the `Primitive`
/// tag — a single render queue can now mix cubes and spheres,
/// dispatched as separate instanced draw calls in `play3d::render`.
#[derive(Debug, Clone, Copy)]
pub struct DrawCall3d {
    pub primitive: Primitive,
    pub at: [f32; 3],
    pub color: [f32; 4],
    pub size: f32,
    /// Phase 17 session 3: interned texture path id, or 0 for the
    /// white fallback (an untextured / tint-only draw). Applies to
    /// cube / sphere / mesh uniformly.
    pub texture: u32,
}

/// The mesh shape behind a `DrawCall3d`. Each variant has its own
/// vertex/index buffer in `play3d`; a frame's queue is partitioned
/// per-primitive and each subset becomes one instanced draw call.
///
/// `Mesh(id)` carries an interned-path id assigned by
/// `Env::intern_mesh_path`. The render side keeps a parallel
/// `HashMap<u32, GpuMesh>` cache; first sight of an id triggers a
/// lazy `.glb` load + GPU upload. v0.2 session 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Primitive {
    Cube,
    Sphere,
    Mesh(u32),
}

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
            bindings: HashMap::new(),
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
        self.bindings.get(name).cloned()
    }

    pub fn set(&mut self, name: String, value: TaggedValue) {
        self.bindings.insert(name, value);
    }

    pub fn contains(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }

    pub fn remove(&mut self, name: &str) {
        self.bindings.remove(name);
    }

    /// Iterate over every (name, value) currently bound. v0.2 Phase
    /// 8.5 session 8f: yields owned `(String, TaggedValue)` tuples.
    pub fn iter_bindings(&self) -> impl Iterator<Item = (String, TaggedValue)> + '_ {
        self.bindings.iter().map(|(k, v)| (k.clone(), *v))
    }

    /// v0.2 Phase 8.5 session 8h: walk every GC root reachable through
    /// this env and mark them. Called from a safepoint inside
    /// `gc_collect_with(|| env.scan_roots())`.
    ///
    /// Roots: bindings (every global), self_value, returning, the
    /// active scene's instance fields + fiber frames, and every
    /// active entity's instance fields + fiber frames.
    pub fn scan_roots(&self) {
        for v in self.bindings.values() {
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
    for frame in &inst.fiber_frames {
        if let FrameKind::Function {
            saved_returning,
            saved_params,
            ..
        } = &frame.kind
        {
            if let Some(v) = saved_returning {
                crate::heap::mark_value(v);
            }
            for (_, slot) in saved_params {
                if let Some(v) = slot {
                    crate::heap::mark_value(v);
                }
            }
        }
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
