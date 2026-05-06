//! Module loader for Phase 13. Given a parsed entry program with
//! `Stmt::Import` nodes, walk the import graph from disk and build
//! a `ModuleGraph` ready for cross-module name resolution
//! (session 3) and the rest of the type / runtime stack.
//!
//! Resolution rule (session 2 — single search path, the importing
//! file's directory):
//!
//!   `import "<logical>"` from `dir/foo.twe` resolves to
//!   `dir/<logical>.twe` after normalising forward slashes.
//!
//! Session 4 generalises this to a search-path list pulled from
//! `twe.toml`. Until then, the resolver is intentionally narrow so
//! the failure modes are obvious.
//!
//! The loader detects cycles and rejects them with a chain in the
//! error message — `a.twe` → `b.twe` → `a.twe`. It does *not* try
//! to be clever about partial-evaluation order; cycles are a
//! source-level mistake in v0.7 and the type-system stability that
//! Phase 13 promises depends on a DAG.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ast::{Program, Stmt};
use crate::lexer;
use crate::parser;

/// One loaded module: where it came from and its parsed AST.
#[derive(Debug, Clone)]
pub struct LoadedModule {
    /// The path the importer wrote (e.g. `"math/vec2"`). For the
    /// entry module, an empty string — the entry file isn't
    /// referenced by any `import`.
    pub logical_path: String,
    /// The resolved on-disk path. Canonicalised when the file
    /// exists; otherwise the joined path the resolver computed
    /// (so error messages still point somewhere readable).
    pub canonical_path: PathBuf,
    pub program: Program,
}

/// The full import graph: the entry plus every transitively
/// imported module. `deps` is keyed by `canonical_path` (as a
/// string) so revisiting the same file from two different importers
/// shares the parsed AST.
#[derive(Debug, Clone)]
pub struct ModuleGraph {
    pub entry: LoadedModule,
    pub deps: BTreeMap<String, LoadedModule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError {
    pub message: String,
    pub help: Option<String>,
    /// Path of the file the error was raised from (the *importer*,
    /// not the missing target — points the user at where to fix).
    pub source: PathBuf,
    pub line: u32,
    pub col: u32,
}

/// Load the entry file plus every transitively imported module.
///
/// The optional `source` argument is the entry file's contents as
/// already-read text; pass `None` to have the loader read it from
/// disk. This lets `twec play` / `twec build` reuse a string
/// they've already loaded (e.g. for hot-reload) rather than
/// touching disk twice.
pub fn load_from_path(entry_path: &Path, source: Option<&str>) -> Result<ModuleGraph, LoadError> {
    let mut loader = Loader::new();
    let entry = loader.load_entry(entry_path, source)?;
    Ok(ModuleGraph {
        entry,
        deps: loader.modules,
    })
}

struct Loader {
    modules: BTreeMap<String, LoadedModule>,
    /// Set of canonical paths currently being loaded, kept in
    /// insertion order so a cycle error can echo the chain back
    /// to the user.
    in_flight: Vec<PathBuf>,
}

impl Loader {
    fn new() -> Self {
        Self {
            modules: BTreeMap::new(),
            in_flight: Vec::new(),
        }
    }

    fn load_entry(
        &mut self,
        entry_path: &Path,
        source: Option<&str>,
    ) -> Result<LoadedModule, LoadError> {
        let canonical = canonicalize_or(entry_path);
        let text = match source {
            Some(s) => s.to_string(),
            None => std::fs::read_to_string(entry_path).map_err(|e| LoadError {
                message: format!("failed to read entry file: {e}"),
                help: None,
                source: entry_path.to_path_buf(),
                line: 1,
                col: 1,
            })?,
        };
        let program = parse_text(&text, entry_path)?;
        self.in_flight.push(canonical.clone());
        self.walk_imports(&canonical, &program)?;
        self.in_flight.pop();
        Ok(LoadedModule {
            logical_path: String::new(),
            canonical_path: canonical,
            program,
        })
    }

    fn walk_imports(&mut self, importer: &Path, program: &Program) -> Result<(), LoadError> {
        for stmt in &program.stmts {
            if let Stmt::Import {
                path, line, col, ..
            } = stmt
            {
                let target = resolve(importer, path).map_err(|message| LoadError {
                    message,
                    help: Some(format!(
                        "Phase 13 session 2 resolves `import \"{path}\"` against the importing file's directory"
                    )),
                    source: importer.to_path_buf(),
                    line: *line,
                    col: *col,
                })?;

                if self.in_flight.iter().any(|p| p == &target) {
                    let chain: Vec<String> =
                        self.in_flight.iter().map(|p| display_path(p)).collect();
                    return Err(LoadError {
                        message: format!(
                            "import cycle detected: {} -> {}",
                            chain.join(" -> "),
                            display_path(&target)
                        ),
                        help: Some(
                            "modules must form a DAG; break the cycle by extracting the shared surface into a third module"
                                .to_string(),
                        ),
                        source: importer.to_path_buf(),
                        line: *line,
                        col: *col,
                    });
                }

                let key = canonical_key(&target);
                if self.modules.contains_key(&key) {
                    continue;
                }

                let text = std::fs::read_to_string(&target).map_err(|e| LoadError {
                    message: format!(
                        "failed to read module `{}` ({}): {e}",
                        path,
                        display_path(&target)
                    ),
                    help: Some(format!(
                        "expected the module at {}",
                        display_path(&target)
                    )),
                    source: importer.to_path_buf(),
                    line: *line,
                    col: *col,
                })?;
                let dep_program = parse_text(&text, &target)?;

                self.in_flight.push(target.clone());
                self.walk_imports(&target, &dep_program)?;
                self.in_flight.pop();

                self.modules.insert(
                    key,
                    LoadedModule {
                        logical_path: path.clone(),
                        canonical_path: target,
                        program: dep_program,
                    },
                );
            }
        }
        Ok(())
    }
}

/// Resolve `logical` relative to `importer`. Returns the resolved
/// path canonicalised when the file exists, otherwise the joined
/// path so the caller can still report a sensible error location.
pub fn resolve(importer: &Path, logical: &str) -> Result<PathBuf, String> {
    if logical.is_empty() {
        return Err("import path is empty".to_string());
    }
    if logical.starts_with('/') || logical.contains("..") {
        return Err(format!(
            "import path `{logical}` must be a relative module name without `..` or leading `/`"
        ));
    }
    let importer_dir = importer.parent().unwrap_or_else(|| Path::new("."));
    let mut joined = PathBuf::from(importer_dir);
    for segment in logical.split('/') {
        if segment.is_empty() {
            return Err(format!(
                "import path `{logical}` has an empty segment (consecutive `/`)"
            ));
        }
        joined.push(segment);
    }
    let mut with_ext = joined.clone();
    with_ext.set_extension("twe");
    if with_ext.exists() {
        return Ok(canonicalize_or(&with_ext));
    }
    Err(format!(
        "module file not found at {}",
        display_path(&with_ext)
    ))
}

fn parse_text(text: &str, file: &Path) -> Result<Program, LoadError> {
    let tokens = lexer::lex(text).map_err(|e| LoadError {
        message: format!("lex error in {}: {}", display_path(file), e.message),
        help: e.help.clone(),
        source: file.to_path_buf(),
        line: e.line,
        col: e.col,
    })?;
    parser::parse(&tokens).map_err(|e| LoadError {
        message: format!("parse error in {}: {}", display_path(file), e.message),
        help: e.help.clone(),
        source: file.to_path_buf(),
        line: e.line,
        col: e.col,
    })
}

fn canonicalize_or(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

pub fn canonical_key(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Topologically sort the dependency map so leaves (no further
/// imports) come first. Cycles were already rejected at load time,
/// so a depth-first walk with a visited set is sufficient. Result
/// is a list of canonical-key strings in the order they should be
/// evaluated.
pub fn topo_order(graph: &ModuleGraph) -> Vec<String> {
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut order: Vec<String> = Vec::new();

    fn visit(
        node_path: &Path,
        node_program: &Program,
        graph: &ModuleGraph,
        visited: &mut std::collections::HashSet<String>,
        order: &mut Vec<String>,
    ) {
        let key = canonical_key(node_path);
        if !visited.insert(key.clone()) {
            return;
        }
        for stmt in &node_program.stmts {
            if let Stmt::Import { path, .. } = stmt {
                if let Ok(target) = resolve(node_path, path) {
                    let dep_key = canonical_key(&target);
                    if let Some(dep) = graph.deps.get(&dep_key) {
                        visit(&dep.canonical_path, &dep.program, graph, visited, order);
                    }
                }
            }
        }
        // Skip the entry — it's not in `deps`. Caller runs it last
        // explicitly.
        if graph.deps.contains_key(&key) {
            order.push(key);
        }
    }

    visit(
        &graph.entry.canonical_path,
        &graph.entry.program,
        graph,
        &mut visited,
        &mut order,
    );
    order
}

/// Snapshot the names that are present in `env` after stdlib
/// installation but before any user code runs. Used by
/// `evaluate_graph` to compute which bindings count as
/// "module-defined" (and therefore become fields of the module
/// value) versus stdlib-inherited (which don't).
pub fn snapshot_stdlib_names(env: &crate::value::Env) -> std::collections::HashSet<String> {
    env.iter_bindings().map(|(k, _)| k).collect()
}

/// Build the module value for one already-evaluated module env.
/// Walks the env's bindings and collects everything that wasn't
/// in the stdlib snapshot. Returns an `Object { kind: "module" }`
/// wrapped in a `TaggedValue` ready to drop into the cache.
pub fn build_module_value(
    env: &crate::value::Env,
    stdlib_names: &std::collections::HashSet<String>,
) -> crate::tagged_value::TaggedValue {
    use std::cell::RefCell;
    use std::rc::Rc;
    let mut fields: std::collections::HashMap<String, crate::tagged_value::TaggedValue> =
        std::collections::HashMap::new();
    for (name, value) in env.iter_bindings() {
        if stdlib_names.contains(&name) {
            continue;
        }
        fields.insert(name, value);
    }
    crate::tagged_value::TaggedValue::from_object(Rc::new(RefCell::new(crate::value::Object {
        fields,
        kind: "module",
    })))
}

/// Compute the binding name for an import: explicit `as Alias`
/// wins; otherwise the last forward-slash segment of the logical
/// path. Empty segments were rejected at resolve time.
pub fn import_binding_name(path: &str, alias: Option<&str>) -> String {
    if let Some(name) = alias {
        return name.to_string();
    }
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Phase 13 session 3 top-level runner. Evaluates every dependency
/// in topological order, builds a module value for each, then runs
/// the entry program against an env whose `module_cache` carries
/// those values. Returns the entry env's `out` buffer so the caller
/// can assert on the program's output, mirroring `eval::run`.
///
/// Each module is evaluated in its own fresh env. Stdlib state is
/// not shared — that's a feature, not a bug: a module's `let
/// counter = 0` is private to that module's file.
pub fn run_with_modules(graph: &ModuleGraph) -> Result<String, crate::value::RuntimeError> {
    let order = topo_order(graph);
    let mut module_cache: std::collections::HashMap<String, crate::tagged_value::TaggedValue> =
        std::collections::HashMap::new();

    for key in &order {
        let module = graph.deps.get(key).expect("topo_order returns only deps");
        let mut sub_env = crate::value::Env::new();
        crate::stdlib::install(&mut sub_env);
        let stdlib_names = snapshot_stdlib_names(&sub_env);
        // Sub-modules carry the full cache built so far so a
        // module's own `import` statement can resolve its deps.
        sub_env.module_cache = module_cache.clone();
        sub_env.current_source = Some(module.canonical_path.clone());
        crate::eval::run_top_level(&mut sub_env, &module.program)?;
        let module_value = build_module_value(&sub_env, &stdlib_names);
        module_cache.insert(key.clone(), module_value);
    }

    let mut env = crate::value::Env::new();
    crate::stdlib::install(&mut env);
    env.module_cache = module_cache;
    env.current_source = Some(graph.entry.canonical_path.clone());
    crate::eval::run_top_level(&mut env, &graph.entry.program)?;
    Ok(env.out)
}

fn display_path(p: &Path) -> String {
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir -p");
        }
        std::fs::write(&path, body).expect("write module");
    }

    fn tmp(name: &str) -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!(
            "twec-module-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn loads_entry_with_one_dep() {
        let dir = tmp("one_dep");
        write(&dir, "main.twe", "import \"helper\"\nlet x = 1\n");
        write(&dir, "helper.twe", "let y = 2\n");
        let g = load_from_path(&dir.join("main.twe"), None).unwrap();
        assert!(g.entry.canonical_path.ends_with("main.twe"));
        assert_eq!(g.deps.len(), 1);
        let helper = g.deps.values().next().unwrap();
        assert_eq!(helper.logical_path, "helper");
        assert!(helper.canonical_path.ends_with("helper.twe"));
    }

    #[test]
    fn deduplicates_diamond_imports() {
        // a imports b and c; both b and c import shared. shared.twe
        // should appear in deps once, not twice.
        let dir = tmp("diamond");
        write(
            &dir,
            "main.twe",
            "import \"b\"\nimport \"c\"\nlet x = 1\n",
        );
        write(&dir, "b.twe", "import \"shared\"\nlet b = 1\n");
        write(&dir, "c.twe", "import \"shared\"\nlet c = 1\n");
        write(&dir, "shared.twe", "let s = 0\n");
        let g = load_from_path(&dir.join("main.twe"), None).unwrap();
        // 3 modules: b, c, shared.
        assert_eq!(g.deps.len(), 3);
    }

    #[test]
    fn rejects_cycle() {
        let dir = tmp("cycle");
        write(&dir, "main.twe", "import \"a\"\n");
        write(&dir, "a.twe", "import \"b\"\n");
        write(&dir, "b.twe", "import \"a\"\n");
        let err = load_from_path(&dir.join("main.twe"), None).unwrap_err();
        assert!(
            err.message.contains("cycle"),
            "expected cycle in error: {}",
            err.message
        );
    }

    #[test]
    fn rejects_dotdot_path() {
        let dir = tmp("escape");
        write(&dir, "main.twe", "import \"../escape\"\n");
        let err = load_from_path(&dir.join("main.twe"), None).unwrap_err();
        assert!(
            err.message.contains(".."),
            "expected `..` rejection: {}",
            err.message
        );
    }

    #[test]
    fn rejects_missing_module() {
        let dir = tmp("missing");
        write(&dir, "main.twe", "import \"ghost\"\n");
        let err = load_from_path(&dir.join("main.twe"), None).unwrap_err();
        assert!(
            err.message.contains("module file not found"),
            "expected missing-file message: {}",
            err.message
        );
    }

    #[test]
    fn entry_with_no_imports_loads_clean() {
        let dir = tmp("noimports");
        write(&dir, "main.twe", "let x = 1\n");
        let g = load_from_path(&dir.join("main.twe"), None).unwrap();
        assert!(g.deps.is_empty());
    }

    #[test]
    fn source_argument_overrides_disk_read() {
        let dir = tmp("source_arg");
        write(&dir, "main.twe", "let on_disk = 0\n");
        let g = load_from_path(
            &dir.join("main.twe"),
            Some("let from_caller = 1\n"),
        )
        .unwrap();
        // The entry's program comes from the caller-supplied text,
        // not the on-disk version. Easiest check is via printer.
        let printed = crate::printer::print_program(&g.entry.program);
        assert!(printed.contains("from_caller"), "got: {printed}");
        assert!(!printed.contains("on_disk"), "got: {printed}");
    }

    // ----- Phase 13 session 3: cross-module name resolution. -----

    #[test]
    fn import_binding_name_uses_basename() {
        assert_eq!(import_binding_name("math", None), "math");
        assert_eq!(import_binding_name("math/vec2", None), "vec2");
        assert_eq!(
            import_binding_name("physics/forces", Some("Forces")),
            "Forces"
        );
    }

    #[test]
    fn topo_order_puts_leaves_first() {
        // main -> a -> shared
        // main -> b -> shared
        // Expected: shared, then a/b (in some order), entry not included.
        let dir = tmp("topo");
        write(
            &dir,
            "main.twe",
            "import \"a\"\nimport \"b\"\n",
        );
        write(&dir, "a.twe", "import \"shared\"\nlet a = 1\n");
        write(&dir, "b.twe", "import \"shared\"\nlet b = 1\n");
        write(&dir, "shared.twe", "let s = 0\n");
        let g = load_from_path(&dir.join("main.twe"), None).unwrap();
        let order = topo_order(&g);
        // shared must come before a and b.
        let shared_idx = order
            .iter()
            .position(|k| k.ends_with("shared.twe"))
            .expect("shared in order");
        let a_idx = order
            .iter()
            .position(|k| k.ends_with("a.twe"))
            .expect("a in order");
        let b_idx = order
            .iter()
            .position(|k| k.ends_with("b.twe"))
            .expect("b in order");
        assert!(shared_idx < a_idx, "shared before a: {order:?}");
        assert!(shared_idx < b_idx, "shared before b: {order:?}");
        // Entry is not included (it's run separately by the runner).
        assert!(
            !order.iter().any(|k| k.ends_with("main.twe")),
            "entry not in topo order: {order:?}"
        );
    }

    #[test]
    fn run_with_modules_makes_imported_function_callable() {
        // The headline use case: a `math.twe` defines a function,
        // the entry imports it and calls `math.add(2, 3)`.
        let dir = tmp("call_imported_fn");
        write(
            &dir,
            "main.twe",
            "import \"math\"\nprint(math.add(2, 3))\n",
        );
        write(
            &dir,
            "math.twe",
            "function add(a, b):\n    return a + b\n",
        );
        let g = load_from_path(&dir.join("main.twe"), None).unwrap();
        let out = run_with_modules(&g).expect("run");
        assert!(out.contains('5'), "expected 5 in output: {out:?}");
    }

    #[test]
    fn run_with_modules_alias_renames_binding() {
        let dir = tmp("alias");
        write(
            &dir,
            "main.twe",
            "import \"math\" as M\nprint(M.add(7, 8))\n",
        );
        write(
            &dir,
            "math.twe",
            "function add(a, b):\n    return a + b\n",
        );
        let g = load_from_path(&dir.join("main.twe"), None).unwrap();
        let out = run_with_modules(&g).expect("run");
        assert!(out.contains("15"), "expected 15 in output: {out:?}");
    }

    #[test]
    fn run_with_modules_module_state_is_isolated() {
        // Each module gets its own env; a `let counter = 0` in one
        // module isn't visible in another except via the module
        // value's exposed surface. Verifies `current_source` plus
        // the snapshot_stdlib_names filter does the right thing.
        let dir = tmp("isolation");
        write(
            &dir,
            "main.twe",
            "import \"counter\"\nprint(counter.value)\n",
        );
        write(&dir, "counter.twe", "let value = 42\n");
        let g = load_from_path(&dir.join("main.twe"), None).unwrap();
        let out = run_with_modules(&g).expect("run");
        assert!(out.contains("42"), "expected 42 in output: {out:?}");
    }

    #[test]
    fn run_with_modules_transitive_imports_resolve() {
        // a imports b, main imports a. main can reach b's surface
        // only via a's re-export; this test instead verifies that
        // a's *use* of b at module-init time works.
        let dir = tmp("transitive");
        write(
            &dir,
            "main.twe",
            "import \"a\"\nprint(a.computed)\n",
        );
        write(
            &dir,
            "a.twe",
            "import \"b\"\nlet computed = b.base * 10\n",
        );
        write(&dir, "b.twe", "let base = 7\n");
        let g = load_from_path(&dir.join("main.twe"), None).unwrap();
        let out = run_with_modules(&g).expect("run");
        assert!(out.contains("70"), "expected 70 in output: {out:?}");
    }
}
