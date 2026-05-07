// Phase 12 session 1: `twec build` skeleton + project layout convention.
//
// This module owns the orchestration of the build pipeline. Sessions 2+
// fill in actual bundle production / binary output / per-config
// behavior; session 1 ships the skeleton — CLI entry, project
// discovery, validation pass — and a dry-run that prints "would
// bundle N files" so the surface is exercisable end-to-end before
// any real artifact gets produced.
//
// Project layout convention (session 1):
//
//   <project_dir>/
//     main.twe          (REQUIRED — entry script)
//     assets/           (OPTIONAL — recursively walked; every file
//                        becomes a bundle entry under its
//                        relative path)
//     twe.toml          (OPTIONAL — manifest; session 5 reads it)
//
// The build subcommand fails fast on parse / type errors so the
// later (slow) bundling sessions only run on known-good inputs.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuildTarget {
    WindowsX86_64,
    MacOsAarch64,
    MacOsX86_64,
    LinuxX86_64,
}

impl BuildTarget {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "windows-x86_64" | "x86_64-pc-windows-msvc" | "x86_64-pc-windows-gnu" => {
                Some(BuildTarget::WindowsX86_64)
            }
            "macos-aarch64" | "aarch64-apple-darwin" => Some(BuildTarget::MacOsAarch64),
            "macos-x86_64" | "x86_64-apple-darwin" => Some(BuildTarget::MacOsX86_64),
            "linux-x86_64" | "x86_64-unknown-linux-gnu" | "x86_64-unknown-linux-musl" => {
                Some(BuildTarget::LinuxX86_64)
            }
            _ => None,
        }
    }

    /// The target the running `twec` was compiled for. `twec build`
    /// defaults to the host target so a contributor running it
    /// without `--target` gets a binary they can actually launch.
    pub fn host() -> Self {
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            BuildTarget::WindowsX86_64
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            BuildTarget::MacOsAarch64
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            BuildTarget::MacOsX86_64
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            BuildTarget::LinuxX86_64
        }
        // No good default if the host is something exotic; fall back
        // to Windows which is also v1.0's lead platform. Users on
        // exotic hosts must pass `--target` explicitly.
        #[cfg(not(any(
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
        )))]
        {
            BuildTarget::WindowsX86_64
        }
    }

    pub fn binary_extension(self) -> &'static str {
        match self {
            BuildTarget::WindowsX86_64 => ".exe",
            BuildTarget::MacOsAarch64 | BuildTarget::MacOsX86_64 | BuildTarget::LinuxX86_64 => "",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BuildTarget::WindowsX86_64 => "windows-x86_64",
            BuildTarget::MacOsAarch64 => "macos-aarch64",
            BuildTarget::MacOsX86_64 => "macos-x86_64",
            BuildTarget::LinuxX86_64 => "linux-x86_64",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuildConfig {
    Dev,
    Release,
    Profile,
}

impl BuildConfig {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "dev" | "debug" => Some(BuildConfig::Dev),
            "release" => Some(BuildConfig::Release),
            "profile" => Some(BuildConfig::Profile),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BuildConfig::Dev => "dev",
            BuildConfig::Release => "release",
            BuildConfig::Profile => "profile",
        }
    }
}

/// Phase 12 session 5: parsed `twe.toml`. All fields optional — a
/// project doesn't have to ship a manifest, and one that does can
/// fill in any subset.
#[derive(Clone, Debug, Default)]
pub struct ProjectManifest {
    pub project_name: Option<String>,
    pub default_target: Option<BuildTarget>,
    pub default_config: Option<BuildConfig>,
    /// Per-config override map keyed on lowercased config label
    /// (`"dev"` / `"release"` / `"profile"`). Configs not mentioned
    /// in the manifest fall back to `ConfigOverride::default()` and
    /// then to the builtin per-config defaults.
    pub configs: std::collections::HashMap<String, ConfigOverride>,
    /// Phase 12 session 9: Steam Depot integration. Populated from
    /// `[steam]` in `twe.toml`. When present + `--steam` is passed
    /// (or `[steam] enabled = true`), `twec build` produces a
    /// Depot-shaped layout next to the regular artifact.
    pub steam: Option<SteamManifest>,
    /// Phase 13 session 4: `[dependencies]` table. Each entry maps a
    /// logical dependency name to a `Dependency { version, path }`.
    /// When `import "<name>/..."` is resolved, the loader prepends
    /// `dependency.path` (if set) to the import-relative search.
    /// Version is metadata for now — actual fetching / verification
    /// is post-v1.0; v0.7's exit criterion is "the layout works",
    /// not "the registry exists".
    pub dependencies: std::collections::HashMap<String, Dependency>,
}

/// Phase 13 session 4: one `[dependencies]` entry. TOML accepts
/// either a string (version pin: `mathlib = "1.2.3"`) or an inline
/// table (`mathlib = { version = "1.2.3", path = "vendor/mathlib" }`).
/// Both forms parse to this struct with the unspecified field left
/// `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dependency {
    pub version: Option<String>,
    pub path: Option<PathBuf>,
}

/// Phase 12 session 9: per-project Steam configuration. App and
/// depot ids default to Spacewar (`480` / `481`) — Valve's public
/// test app — so a project that hasn't claimed real ids yet still
/// produces a layout that round-trips through `steamcmd`.
#[derive(Clone, Debug, Default)]
pub struct SteamManifest {
    pub enabled: bool,
    pub app_id: Option<u32>,
    pub depot_id: Option<u32>,
    pub depot_description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigOverride {
    pub hot_reload: Option<bool>,
    pub bundle_assets: Option<bool>,
    pub strip_debug: Option<bool>,
    pub profile: Option<bool>,
    /// Phase 12 session 8: opt into zstd-compressed bundle bodies.
    /// Manifest key `compress` (`[build.<config>] compress = true`).
    pub compress: Option<bool>,
}

/// Resolved per-config flags. The build pipeline reads this after
/// merging (a) the builtin defaults for the chosen config, (b) any
/// `[build.<config>]` overrides from `twe.toml`. CLI flags don't
/// override these today; if a project pressures it, an explicit
/// `--no-bundle` / `--bundle` / `--profile` set lands as a follow-on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub config: BuildConfig,
    pub hot_reload: bool,
    pub bundle_assets: bool,
    pub strip_debug: bool,
    pub profile: bool,
    /// Phase 12 session 8: zstd-compress bundle bodies. `dev` skips
    /// it (faster builds, no ratio benefit during inner-loop), `release`
    /// and `profile` enable.
    pub compress: bool,
}

impl ResolvedConfig {
    /// Builtin per-config defaults. Manifest overrides apply on
    /// top via `apply_override`.
    pub fn defaults_for(config: BuildConfig) -> Self {
        match config {
            BuildConfig::Dev => Self {
                config,
                // Dev config means "produce an .exe that behaves
                // like a developer-loop launch": assets stay on
                // disk so they're editable without rebuilding;
                // hot reload tracks main.twe alongside the .exe.
                hot_reload: true,
                bundle_assets: false,
                strip_debug: false,
                profile: false,
                compress: false,
            },
            BuildConfig::Release => Self {
                config,
                hot_reload: false,
                bundle_assets: true,
                strip_debug: true,
                profile: false,
                compress: true,
            },
            BuildConfig::Profile => Self {
                config,
                hot_reload: false,
                bundle_assets: true,
                strip_debug: true,
                profile: true,
                compress: true,
            },
        }
    }

    pub fn apply_override(&mut self, ovr: &ConfigOverride) {
        if let Some(v) = ovr.hot_reload {
            self.hot_reload = v;
        }
        if let Some(v) = ovr.bundle_assets {
            self.bundle_assets = v;
        }
        if let Some(v) = ovr.strip_debug {
            self.strip_debug = v;
        }
        if let Some(v) = ovr.profile {
            self.profile = v;
        }
        if let Some(v) = ovr.compress {
            self.compress = v;
        }
    }
}

/// Parse a `twe.toml` from disk. Errors carry the file path so
/// diagnostics are clickable. Unknown keys are ignored — the manifest
/// surface evolves over phases and old projects shouldn't break on
/// new keys.
pub fn parse_manifest(path: &Path) -> Result<ProjectManifest, String> {
    let display = path.display().to_string();
    let src = fs::read_to_string(path).map_err(|e| format!("cannot read '{display}': {e}"))?;
    let value: toml::Value = src
        .parse()
        .map_err(|e| format!("{display}: invalid TOML: {e}"))?;
    let mut manifest = ProjectManifest::default();
    if let Some(project) = value.get("project").and_then(|v| v.as_table()) {
        if let Some(name) = project.get("name").and_then(|v| v.as_str()) {
            manifest.project_name = Some(name.to_string());
        }
    }
    if let Some(build) = value.get("build").and_then(|v| v.as_table()) {
        if let Some(t) = build.get("default_target").and_then(|v| v.as_str()) {
            let parsed = BuildTarget::parse(t).ok_or_else(|| {
                format!("{display}: build.default_target = '{t}' is not a known target")
            })?;
            manifest.default_target = Some(parsed);
        }
        if let Some(c) = build.get("default_config").and_then(|v| v.as_str()) {
            let parsed = BuildConfig::parse(c).ok_or_else(|| {
                format!("{display}: build.default_config = '{c}' is not a known config")
            })?;
            manifest.default_config = Some(parsed);
        }
        for (key, val) in build.iter() {
            // Sub-tables `[build.dev]` / `[build.release]` /
            // `[build.profile]` carry per-config overrides.
            let Some(tbl) = val.as_table() else { continue };
            if BuildConfig::parse(key).is_none() {
                continue;
            }
            let ovr = ConfigOverride {
                hot_reload: tbl.get("hot_reload").and_then(|v| v.as_bool()),
                bundle_assets: tbl.get("bundle_assets").and_then(|v| v.as_bool()),
                strip_debug: tbl.get("strip_debug").and_then(|v| v.as_bool()),
                profile: tbl.get("profile").and_then(|v| v.as_bool()),
                compress: tbl.get("compress").and_then(|v| v.as_bool()),
            };
            manifest.configs.insert(key.to_string(), ovr);
        }
    }
    if let Some(steam) = value.get("steam").and_then(|v| v.as_table()) {
        let mut s = SteamManifest::default();
        if let Some(b) = steam.get("enabled").and_then(|v| v.as_bool()) {
            s.enabled = b;
        }
        if let Some(id) = steam.get("app_id").and_then(|v| v.as_integer()) {
            if id < 0 || id > u32::MAX as i64 {
                return Err(format!(
                    "{display}: steam.app_id = {id} is out of range (0..{})",
                    u32::MAX
                ));
            }
            s.app_id = Some(id as u32);
        }
        if let Some(id) = steam.get("depot_id").and_then(|v| v.as_integer()) {
            if id < 0 || id > u32::MAX as i64 {
                return Err(format!(
                    "{display}: steam.depot_id = {id} is out of range (0..{})",
                    u32::MAX
                ));
            }
            s.depot_id = Some(id as u32);
        }
        if let Some(d) = steam.get("depot_description").and_then(|v| v.as_str()) {
            s.depot_description = Some(d.to_string());
        }
        manifest.steam = Some(s);
    }
    if let Some(deps) = value.get("dependencies").and_then(|v| v.as_table()) {
        // Phase 13 session 4. TOML allows two shapes per entry:
        //   foo = "1.2.3"
        //   foo = { version = "1.2.3", path = "vendor/foo" }
        for (name, entry) in deps.iter() {
            let dep = match entry {
                toml::Value::String(s) => Dependency {
                    version: Some(s.clone()),
                    path: None,
                },
                toml::Value::Table(t) => Dependency {
                    version: t
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    path: t.get("path").and_then(|v| v.as_str()).map(PathBuf::from),
                },
                _ => {
                    return Err(format!(
                        "{display}: dependencies.{name} must be a version string or an inline table"
                    ));
                }
            };
            manifest.dependencies.insert(name.clone(), dep);
        }
    }
    Ok(manifest)
}

/// Merge a manifest's per-config override (if any) into the builtin
/// defaults for `config`. Manifest wins where it specifies a value;
/// builtin defaults fill the gaps.
pub fn resolve_config(manifest: Option<&ProjectManifest>, config: BuildConfig) -> ResolvedConfig {
    let mut resolved = ResolvedConfig::defaults_for(config);
    if let Some(m) = manifest {
        if let Some(ovr) = m.configs.get(config.label()) {
            resolved.apply_override(ovr);
        }
    }
    resolved
}

#[derive(Clone, Debug)]
pub struct BuildArgs {
    pub project_dir: PathBuf,
    pub target: BuildTarget,
    /// True when `--target` was passed on the CLI; defaults from
    /// `twe.toml` only apply when this is false.
    pub target_explicit: bool,
    pub config: BuildConfig,
    /// Same signal for `--config`.
    pub config_explicit: bool,
    pub out: Option<PathBuf>,
    pub dry_run: bool,
    /// Phase 12 session 9: when true, `twec build` produces a Steam
    /// Depot layout (`<dist>/<name>.steam/`) next to the per-target
    /// artifact. CLI flag `--steam`; manifest equivalent
    /// `[steam] enabled = true`.
    pub steam: bool,
}

#[derive(Debug)]
pub struct DiscoveredProject {
    pub root: PathBuf,
    /// Absolute path to `main.twe`.
    pub main: PathBuf,
    /// Project name — directory name. Used as the default output
    /// binary stem.
    pub name: String,
    /// All asset files (absolute paths), recursively walked from
    /// `<root>/assets/` if it exists. Empty when there's no assets/
    /// dir. Sorted by relative path for reproducible builds.
    pub assets: Vec<AssetEntry>,
    /// `twe.toml` if it exists. Session 5 parses it.
    pub manifest: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct AssetEntry {
    /// Absolute path on disk.
    pub abs: PathBuf,
    /// Path relative to the project root, using forward slashes —
    /// this is what becomes the bundle key in session 2 and what
    /// scripts pass to the loaders. Always starts with `assets/`.
    pub bundle_key: String,
}

pub fn discover_project(dir: &Path) -> Result<DiscoveredProject, String> {
    let root = dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve project directory '{}': {e}", dir.display()))?;
    if !root.is_dir() {
        return Err(format!(
            "project path '{}' is not a directory",
            dir.display()
        ));
    }
    let main = root.join("main.twe");
    if !main.is_file() {
        return Err(format!(
            "project '{}' is missing required file 'main.twe'",
            root.display()
        ));
    }
    let name = root
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "twe_game".to_string());
    let manifest_path = root.join("twe.toml");
    let manifest = if manifest_path.is_file() {
        Some(manifest_path)
    } else {
        None
    };
    let assets_dir = root.join("assets");
    let mut assets = Vec::new();
    if assets_dir.is_dir() {
        walk_assets(&assets_dir, &root, &mut assets)?;
        assets.sort_by(|a, b| a.bundle_key.cmp(&b.bundle_key));
    }
    Ok(DiscoveredProject {
        root,
        main,
        name,
        assets,
        manifest,
    })
}

fn walk_assets(dir: &Path, root: &Path, out: &mut Vec<AssetEntry>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("cannot read directory '{}': {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("error walking '{}': {e}", dir.display()))?;
        let path = entry.path();
        let ft = entry
            .file_type()
            .map_err(|e| format!("cannot stat '{}': {e}", path.display()))?;
        if ft.is_dir() {
            walk_assets(&path, root, out)?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| format!("'{}' is outside the project root", path.display()))?;
            // Bundle keys use forward slashes regardless of host
            // platform — Windows builds reading a bundle authored on
            // macOS, and vice versa, must agree on a single
            // canonical form.
            let bundle_key = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(AssetEntry {
                abs: path,
                bundle_key,
            });
        }
        // Symlinks / other types are silently skipped — pulling
        // arbitrary symlink targets into a bundle is a footgun.
    }
    Ok(())
}

/// Parse + typecheck `main.twe` so build failures are caught before
/// any expensive bundling step. Mirrors the lex / parse / infer
/// pipeline `twec types` runs. Returns the parsed program for
/// downstream sessions to reuse.
pub fn validate_project(project: &DiscoveredProject) -> Result<crate::ast::Program, String> {
    let main_str = project.main.display().to_string();
    let src =
        fs::read_to_string(&project.main).map_err(|e| format!("cannot read '{main_str}': {e}"))?;
    let tokens = crate::lexer::lex(&src).map_err(|e| format!("{main_str}:{e}"))?;
    let program = crate::parser::parse(&tokens).map_err(|e| format!("{main_str}:{e}"))?;
    // Phase 4 closed at non-strict default per CLAUDE.md, so type
    // inference runs to populate the bindings table but does *not*
    // gate the build — strict mode is opt-in. Real lex / parse
    // errors above are the only hard build-failure path here. The
    // strict-gate flag (e.g. `--config release-strict`) lands when
    // a contributor wants it; not in scope for v0.6.
    let _bindings = crate::infer::infer_program(&program);
    Ok(program)
}

/// `twec build` CLI entry. Returns the process exit code (0 on
/// success, 2 on argument errors, 1 on build errors).
pub fn run(mut args: BuildArgs) -> i32 {
    let project = match discover_project(&args.project_dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if let Err(e) = validate_project(&project) {
        eprintln!("error: {e}");
        return 1;
    }
    // Phase 12 session 5: parse `twe.toml` if it exists; manifest
    // defaults for `target` / `config` apply only when the CLI
    // *didn't* pass an explicit value (the CLI signal carries
    // `Some(_)` exactly when the flag was set).
    let manifest = if let Some(p) = &project.manifest {
        match parse_manifest(p) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    } else {
        None
    };
    if let Some(m) = &manifest {
        if !args.target_explicit {
            if let Some(t) = m.default_target {
                args.target = t;
            }
        }
        if !args.config_explicit {
            if let Some(c) = m.default_config {
                args.config = c;
            }
        }
    }
    let resolved = resolve_config(manifest.as_ref(), args.config);
    let out_path = match resolve_out_path(&project, &args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    eprintln!(
        "[twec build] project: {}  ({} asset{})",
        project.name,
        project.assets.len(),
        if project.assets.len() == 1 { "" } else { "s" }
    );
    eprintln!(
        "[twec build] target:  {}    config: {} \
         (hot_reload={}, bundle_assets={}, strip_debug={}, profile={}, compress={})",
        args.target.label(),
        args.config.label(),
        resolved.hot_reload,
        resolved.bundle_assets,
        resolved.strip_debug,
        resolved.profile,
        resolved.compress,
    );
    eprintln!("[twec build] out:     {}", out_path.display());
    if let Some(m) = &project.manifest {
        eprintln!("[twec build] manifest: {}", m.display());
    }
    if args.dry_run {
        eprintln!(
            "[twec build] dry run — would bundle {} file{} (main.twe + {} asset{})",
            project.assets.len() + 1,
            if project.assets.is_empty() { "" } else { "s" },
            project.assets.len(),
            if project.assets.len() == 1 { "" } else { "s" }
        );
        return 0;
    }
    // Phase 12 session 4: produce a self-extracting `.exe` for
    // Windows (host-only for now — sessions 6 / 7 add macOS / Linux
    // targets, which need their own runtime binaries). For non-
    // Windows targets we still ship the `.twebundle` artifact as a
    // pre-session-6/7 deliverable; that's exactly what session 4's
    // contract says — Windows is the EXIT-GATE platform.
    // Phase 12 session 9: Steam Depot layout. Resolve the `--steam`
    // flag against the manifest's `[steam] enabled` so a project can
    // make Steam packaging the default once they've claimed an app id.
    let steam_enabled = args.steam
        || manifest
            .as_ref()
            .and_then(|m| m.steam.as_ref())
            .map(|s| s.enabled)
            .unwrap_or(false);
    let exit = match args.target {
        BuildTarget::WindowsX86_64 if BuildTarget::host() == BuildTarget::WindowsX86_64 => {
            build_self_extracting(&project, &out_path, &args, &resolved)
        }
        BuildTarget::MacOsAarch64 | BuildTarget::MacOsX86_64 => {
            build_macos_app(&project, &out_path, &args, &resolved)
        }
        BuildTarget::LinuxX86_64 => build_linux_appdir(&project, &out_path, &args, &resolved),
        #[allow(unreachable_patterns)]
        _ => {
            // Standalone bundle as the universal fallback. Today
            // every BuildTarget variant has its own arm above; this
            // arm exists so future targets land safely.
            let bundle_out = bundle_out_path(&out_path);
            let opts = crate::bundle::EncodeOptions {
                compress: resolved.compress,
            };
            match write_bundle_with_options(&project, &bundle_out, opts) {
                Ok(bytes_written) => {
                    eprintln!(
                        "[twec build] wrote bundle: {} ({} bytes, {} entries{})",
                        bundle_out.display(),
                        bytes_written,
                        project.assets.len() + 1,
                        if resolved.compress { ", zstd" } else { "" }
                    );
                    eprintln!(
                        "[twec build] note: real {} binary production lands in a later session; \
                         the .twebundle is the artifact for now",
                        args.target.label()
                    );
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
    };
    if exit == 0 && steam_enabled {
        if let Err(e) = write_steam_layout(&project, &out_path, manifest.as_ref(), &args) {
            eprintln!("error: writing Steam layout: {e}");
            return 1;
        }
    }
    exit
}

/// Phase 12 session 10: read a bundle's provenance + entry list and
/// print a human-readable summary. Accepts either a standalone
/// `.twebundle` or a self-extracting binary (the same probe
/// `cli::run` does at startup); the path argument tells us which.
pub fn run_info(path: &Path) -> i32 {
    // Try the embedded-bundle path first (works for `.exe` and any
    // future host-runtime layout). If no footer is present, fall
    // back to opening the file as a plain `.twebundle`.
    let mut reader = match crate::bundle::detect_in_file(path) {
        Ok(Some(r)) => r,
        Ok(None) => match crate::bundle::BundleReader::open(path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "error: '{}' is not a twebundle or self-extracting binary: {e}",
                    path.display()
                );
                return 1;
            }
        },
        Err(e) => {
            eprintln!("error: probing '{}': {e}", path.display());
            return 1;
        }
    };
    println!("twec info: {}", path.display());
    println!();
    let prov_bytes = match reader.read(crate::bundle::PROVENANCE_KEY) {
        Ok(Some(b)) => Some(b),
        Ok(None) => None,
        Err(e) => {
            eprintln!("error: reading provenance: {e}");
            return 1;
        }
    };
    if let Some(bytes) = prov_bytes {
        let toml_src = match std::str::from_utf8(&bytes) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("error: provenance entry is not valid UTF-8");
                return 1;
            }
        };
        match crate::bundle::BundleProvenance::from_toml(toml_src) {
            Ok(p) => print_provenance(&p),
            Err(e) => {
                eprintln!("warning: could not parse provenance: {e}");
                println!("(provenance entry present but malformed)");
            }
        }
    } else {
        println!("Provenance: (none — bundle predates session 10 or was hand-built)");
    }
    println!();
    let mut keys: Vec<String> = reader.keys().map(|s| s.to_string()).collect();
    keys.sort();
    let user_count = keys
        .iter()
        .filter(|k| k.as_str() != crate::bundle::PROVENANCE_KEY)
        .count();
    let compressed = reader.header.flags & crate::bundle::FLAG_ZSTD != 0;
    println!(
        "Entries ({user_count}{}):",
        if compressed { ", zstd-compressed" } else { "" }
    );
    for key in &keys {
        if key == crate::bundle::PROVENANCE_KEY {
            continue;
        }
        // Look up the on-disk size from the header.
        let size = reader.header.find(key).map(|e| e.body_length).unwrap_or(0);
        println!("  {key:<40} {size} bytes");
    }
    0
}

fn print_provenance(p: &crate::bundle::BundleProvenance) {
    println!("Provenance:");
    println!("  twec_version:    {}", p.twec_version);
    println!("  built on host:   {}-{}", p.host_os, p.host_arch);
    println!("  built unix-secs: {}", p.build_unix_secs);
    println!("  project:         {}", p.project_name);
    println!("  target:          {}", p.target);
    println!("  config:          {}", p.config);
    println!("  compress:        {}", p.compress);
    println!("  entry_count:     {}", p.entry_count);
}

/// Phase 12 session 9: produce a Steam Depot layout at
/// `<out_path-parent>/<name>.steam/`. Layout follows the `steamcmd`
/// + `depot_build` convention:
///
/// ```text
/// <name>.steam/
///   steam_appid.txt           — used by the Steamworks SDK to
///                                connect a build to its app id
///   app_build_<app_id>.vdf    — `steamcmd app_build` script
///   depot_build_<depot_id>.vdf — depot manifest stub
///   content/                  — DepotContentRoot. The artifact
///                                produced by the per-target build
///                                is what users on `steamcmd` push.
/// ```
///
/// The `content/` directory is created empty (so layout tools can
/// inspect a freshly-built project), and a `README.txt` explains
/// what to drop in. Wiring per-target artifact copies in lands as
/// a follow-on once `examples/survive` actually goes through the
/// layout end-to-end (Phase 12 session 11 — EXIT GATE).
pub fn write_steam_layout(
    project: &DiscoveredProject,
    out_path: &Path,
    manifest: Option<&ProjectManifest>,
    _args: &BuildArgs,
) -> Result<(), String> {
    let parent = out_path.parent().unwrap_or_else(|| Path::new("."));
    let steam_dir = parent.join(format!("{}.steam", project.name));
    let content_dir = steam_dir.join("content");
    fs::create_dir_all(&content_dir)
        .map_err(|e| format!("cannot create '{}': {e}", content_dir.display()))?;

    let steam = manifest.and_then(|m| m.steam.as_ref());
    // Spacewar (480 / 481) is Valve's public test app — running
    // `steamcmd app_build` against it produces no real depot, but
    // the layout round-trips through Steamworks tooling. Real
    // projects override via `[steam] app_id = ...` in twe.toml.
    let app_id = steam.and_then(|s| s.app_id).unwrap_or(480);
    let depot_id = steam.and_then(|s| s.depot_id).unwrap_or(481);
    let description = steam
        .and_then(|s| s.depot_description.clone())
        .unwrap_or_else(|| format!("{} build", project.name));

    fs::write(steam_dir.join("steam_appid.txt"), format!("{app_id}\n"))
        .map_err(|e| format!("cannot write steam_appid.txt: {e}"))?;

    let app_build = render_app_build_vdf(app_id, depot_id, &description);
    fs::write(steam_dir.join(format!("app_build_{app_id}.vdf")), app_build)
        .map_err(|e| format!("cannot write app_build vdf: {e}"))?;

    let depot_build = render_depot_build_vdf(depot_id);
    fs::write(
        steam_dir.join(format!("depot_build_{depot_id}.vdf")),
        depot_build,
    )
    .map_err(|e| format!("cannot write depot_build vdf: {e}"))?;

    let readme = format!(
        "Steam Depot layout for `{name}` (app {app_id}, depot {depot_id}).\n\n\
         Drop the per-target build artifact (the `.exe` / `.app` / `.AppDir`)\n\
         into `content/` before running `steamcmd +run_app_build app_build_{app_id}.vdf`.\n\n\
         Override the app/depot ids by adding `[steam]` to your twe.toml:\n\n\
         \t[steam]\n\
         \tenabled = true\n\
         \tapp_id = 1234560\n\
         \tdepot_id = 1234561\n",
        name = project.name,
    );
    fs::write(steam_dir.join("README.txt"), readme)
        .map_err(|e| format!("cannot write README.txt: {e}"))?;

    eprintln!(
        "[twec build] wrote Steam Depot layout: {} (app {}, depot {})",
        steam_dir.display(),
        app_id,
        depot_id
    );
    Ok(())
}

/// Render the `app_build_<appid>.vdf` Steamworks build script. The
/// VDF format is Valve's KeyValues — a brace-delimited tree of
/// quoted key/value pairs. The minimal app_build includes:
/// `appid`, `desc`, `contentroot`, `buildoutput`, `depots` table.
pub fn render_app_build_vdf(app_id: u32, depot_id: u32, description: &str) -> String {
    let escaped = description.replace('"', "\\\"");
    format!(
        "\"appbuild\"\n\
         {{\n\
         \t\"appid\" \"{app_id}\"\n\
         \t\"desc\" \"{escaped}\"\n\
         \t\"buildoutput\" \"output\"\n\
         \t\"contentroot\" \"content\"\n\
         \t\"setlive\" \"\"\n\
         \t\"preview\" \"0\"\n\
         \t\"local\" \"\"\n\
         \t\"depots\"\n\
         \t{{\n\
         \t\t\"{depot_id}\" \"depot_build_{depot_id}.vdf\"\n\
         \t}}\n\
         }}\n"
    )
}

/// Render the per-depot VDF. A depot manifest at minimum names the
/// content root and the file mapping (`*` = pick up everything).
pub fn render_depot_build_vdf(depot_id: u32) -> String {
    format!(
        "\"DepotBuildConfig\"\n\
         {{\n\
         \t\"DepotID\" \"{depot_id}\"\n\
         \t\"ContentRoot\" \"content\"\n\
         \t\"FileMapping\"\n\
         \t{{\n\
         \t\t\"LocalPath\" \"*\"\n\
         \t\t\"DepotPath\" \".\"\n\
         \t\t\"recursive\" \"1\"\n\
         \t}}\n\
         \t\"FileExclusion\" \"*.pdb\"\n\
         }}\n"
    )
}

/// Phase 12 session 4: produce a single self-extracting `.exe` by
/// (a) encoding the bundle to memory, (b) copying the running
/// `twec.exe` to the output path, (c) appending the bundle bytes +
/// footer. The runtime detects the footer at startup via
/// `bundle::detect_in_self`.
fn build_self_extracting(
    project: &DiscoveredProject,
    out_path: &Path,
    args: &BuildArgs,
    resolved: &ResolvedConfig,
) -> i32 {
    // Encode the bundle to memory (small enough — full survive.twe
    // tree is ~1MB; v0.6 doesn't ship multi-GB games).
    let bundle_bytes =
        match encode_bundle_to_vec(project, resolved.compress, args.target, args.config) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
    let runtime = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot locate twec runtime binary: {e}");
            return 1;
        }
    };
    match crate::bundle::append_to_binary(&runtime, &bundle_bytes, out_path) {
        Ok(bundle_offset) => {
            eprintln!(
                "[twec build] wrote {} (runtime {} bytes + bundle {} bytes; \
                 bundle offset {})",
                out_path.display(),
                bundle_offset,
                bundle_bytes.len(),
                bundle_offset
            );
            0
        }
        Err(e) => {
            eprintln!("error: appending bundle to runtime: {e}");
            1
        }
    }
}

/// Encode a project + provenance into an in-memory bundle. Used by
/// the per-target build paths (Windows / macOS / Linux) and by the
/// session 10 / 11 integration tests. Adds the provenance entry as
/// the trailing record so `entry_count` (user-facing entries) stays
/// honest.
pub fn encode_bundle_to_vec(
    project: &DiscoveredProject,
    compress: bool,
    target: BuildTarget,
    config: BuildConfig,
) -> Result<Vec<u8>, String> {
    let main_bytes = fs::read(&project.main)
        .map_err(|e| format!("cannot read '{}': {e}", project.main.display()))?;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(project.assets.len() + 2);
    entries.push(("main.twe".to_string(), main_bytes));
    for asset in &project.assets {
        let bytes = fs::read(&asset.abs)
            .map_err(|e| format!("cannot read '{}': {e}", asset.abs.display()))?;
        entries.push((asset.bundle_key.clone(), bytes));
    }
    // Phase 12 session 10: append the provenance entry last so the
    // recorded `entry_count` (user-facing entries: main.twe + assets)
    // is the one truth.
    let prov = build_provenance(project, target, config, compress, entries.len() as u32);
    entries.push((
        crate::bundle::PROVENANCE_KEY.to_string(),
        prov.to_toml().into_bytes(),
    ));
    let mut buf = Vec::new();
    let opts = crate::bundle::EncodeOptions { compress };
    crate::bundle::encode_with_options(&mut buf, &entries, opts)
        .map_err(|e| format!("encoding bundle: {e}"))?;
    Ok(buf)
}

fn build_provenance(
    project: &DiscoveredProject,
    target: BuildTarget,
    config: BuildConfig,
    compress: bool,
    entry_count: u32,
) -> crate::bundle::BundleProvenance {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::bundle::BundleProvenance {
        twec_version: env!("CARGO_PKG_VERSION").to_string(),
        host_os: std::env::consts::OS.to_string(),
        host_arch: std::env::consts::ARCH.to_string(),
        build_unix_secs: secs,
        project_name: project.name.clone(),
        target: target.label().to_string(),
        config: config.label().to_string(),
        compress,
        entry_count,
    }
}

/// Output path for the standalone `.twebundle` artifact. Sessions
/// 4+ retire this in favor of a proper binary, but for the
/// session-2 / -3 / -5 milestones it's the artifact users see.
fn bundle_out_path(out_path: &Path) -> PathBuf {
    // If the user passed `--out foo.exe`, write `foo.twebundle`
    // alongside; if they passed `--out foo`, write `foo.twebundle`.
    let mut p = out_path.to_path_buf();
    p.set_extension("twebundle");
    p
}

/// Walk the project + write a `.twebundle` to `out_path`. Returns
/// the number of bytes written. Used both by `twec build` and by
/// the standalone `twec bundle` subcommand. Defaults to no
/// compression — call `write_bundle_with_options` to opt in.
pub fn write_bundle(project: &DiscoveredProject, out_path: &Path) -> Result<u64, String> {
    write_bundle_with_options(project, out_path, crate::bundle::EncodeOptions::default())
}

/// Phase 12 session 8: variant of `write_bundle` that takes
/// `EncodeOptions`. The build pipeline picks compression based on
/// the resolved config; the `twec bundle` subcommand defaults to
/// uncompressed for diff-friendly review.
pub fn write_bundle_with_options(
    project: &DiscoveredProject,
    out_path: &Path,
    opts: crate::bundle::EncodeOptions,
) -> Result<u64, String> {
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create '{}': {e}", parent.display()))?;
        }
    }
    let main_bytes = fs::read(&project.main)
        .map_err(|e| format!("cannot read '{}': {e}", project.main.display()))?;
    // Bundle entry layout: `main.twe` first (so the runtime entry
    // point is found in the same well-known place every time), then
    // assets in their already-sorted bundle-key order.
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(project.assets.len() + 1);
    entries.push(("main.twe".to_string(), main_bytes));
    for asset in &project.assets {
        let bytes = fs::read(&asset.abs)
            .map_err(|e| format!("cannot read '{}': {e}", asset.abs.display()))?;
        entries.push((asset.bundle_key.clone(), bytes));
    }
    let mut file = fs::File::create(out_path)
        .map_err(|e| format!("cannot create '{}': {e}", out_path.display()))?;
    crate::bundle::encode_with_options(&mut file, &entries, opts)
        .map_err(|e| format!("encoding bundle: {e}"))
}

/// Phase 12 session 6: produce a macOS `.app` bundle skeleton at
/// `out_path` (which the caller picks; default is
/// `<project>/dist/<name>.app`). The layout is:
///
/// ```text
/// <name>.app/
///   Contents/
///     Info.plist        — minimal CFBundle keys
///     MacOS/
///       <name>          — runtime binary (host-only) OR <name>.twebundle (cross-compile)
///     Resources/        — empty placeholder; reserved for icon / nibs
/// ```
///
/// When the host is macOS we copy the running `twec` binary into
/// `MacOS/<name>` and append the bundle (mirrors the Windows
/// self-extracting path). When the host is anything else we still
/// produce the directory layout but drop a `.twebundle` next to a
/// stub README — a real .app needs a Mach-O runtime and we don't
/// have a cross-compiled one yet (session 11 / cargo-dist territory).
fn build_macos_app(
    project: &DiscoveredProject,
    out_path: &Path,
    args: &BuildArgs,
    resolved: &ResolvedConfig,
) -> i32 {
    // Force `.app` extension for the output directory so the layout
    // matches macOS expectations even when `--out` skipped it.
    let app_dir = if out_path.extension().and_then(|s| s.to_str()) == Some("app") {
        out_path.to_path_buf()
    } else {
        let mut p = out_path.to_path_buf();
        p.set_extension("app");
        p
    };
    if let Err(e) = fs::create_dir_all(app_dir.join("Contents/MacOS")) {
        eprintln!("error: cannot create '{}': {e}", app_dir.display());
        return 1;
    }
    if let Err(e) = fs::create_dir_all(app_dir.join("Contents/Resources")) {
        eprintln!("error: cannot create Resources dir: {e}");
        return 1;
    }
    let exec_name = project.name.clone();
    let info_plist = render_info_plist(&exec_name, env!("CARGO_PKG_VERSION"));
    let plist_path = app_dir.join("Contents/Info.plist");
    if let Err(e) = fs::write(&plist_path, info_plist) {
        eprintln!("error: cannot write '{}': {e}", plist_path.display());
        return 1;
    }
    let bundle_bytes =
        match encode_bundle_to_vec(project, resolved.compress, args.target, args.config) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
    let exec_path = app_dir.join("Contents/MacOS").join(&exec_name);
    let host = BuildTarget::host();
    let host_is_macos = host == BuildTarget::MacOsAarch64 || host == BuildTarget::MacOsX86_64;
    if host_is_macos {
        let runtime = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: cannot locate twec runtime binary: {e}");
                return 1;
            }
        };
        match crate::bundle::append_to_binary(&runtime, &bundle_bytes, &exec_path) {
            Ok(offset) => {
                eprintln!(
                    "[twec build] wrote {} (runtime {} bytes + bundle {} bytes)",
                    app_dir.display(),
                    offset,
                    bundle_bytes.len()
                );
                0
            }
            Err(e) => {
                eprintln!("error: appending bundle to runtime: {e}");
                1
            }
        }
    } else {
        // Cross-compile: drop the bundle alongside a placeholder
        // README so the layout is recognizable but obviously not
        // signable yet. Producing a real Mach-O requires either
        // (a) a pre-built per-target twec runtime that ships
        // alongside the build, or (b) cargo-dist-driven release
        // artifacts. Either route lands as a follow-on; today's
        // session 6 ships the layout.
        let bundle_path = app_dir
            .join("Contents/MacOS")
            .join(format!("{exec_name}.twebundle"));
        if let Err(e) = fs::write(&bundle_path, &bundle_bytes) {
            eprintln!("error: cannot write '{}': {e}", bundle_path.display());
            return 1;
        }
        let readme = app_dir.join("Contents/MacOS/README.txt");
        let body = format!(
            "This .app skeleton was produced on a non-macOS host \
             ({}). The Mach-O runtime binary is missing; ship the \
             bundle through a macOS host's `twec build` to produce \
             a launchable .app.\n",
            host.label()
        );
        let _ = fs::write(&readme, body);
        eprintln!(
            "[twec build] wrote {} ({} bundle bytes; cross-compile — \
             host '{}' cannot package a Mach-O runtime yet)",
            app_dir.display(),
            bundle_bytes.len(),
            host.label()
        );
        0
    }
}

/// Minimal `Info.plist` for the macOS `.app` skeleton. The plist
/// hand-emit avoids pulling a plist crate in for one file. macOS
/// only requires a small core set of keys to recognize the bundle:
/// `CFBundleExecutable` (the binary in `MacOS/`),
/// `CFBundleIdentifier` (reverse-DNS), `CFBundleName`,
/// `CFBundlePackageType` = `APPL`, plus version keys.
pub fn render_info_plist(exec_name: &str, version: &str) -> String {
    let identifier = format!("dev.twe.{}", sanitize_identifier(exec_name));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>CFBundleExecutable</key>\n\
         \t<string>{exec_name}</string>\n\
         \t<key>CFBundleIdentifier</key>\n\
         \t<string>{identifier}</string>\n\
         \t<key>CFBundleName</key>\n\
         \t<string>{exec_name}</string>\n\
         \t<key>CFBundlePackageType</key>\n\
         \t<string>APPL</string>\n\
         \t<key>CFBundleShortVersionString</key>\n\
         \t<string>{version}</string>\n\
         \t<key>CFBundleVersion</key>\n\
         \t<string>{version}</string>\n\
         \t<key>NSHighResolutionCapable</key>\n\
         \t<true/>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// Phase 12 session 7: produce a Linux AppDir layout at `out_path`.
/// AppDir is the input format for `appimagetool`; producing the
/// directory itself doesn't need any external tool, which is what
/// makes session 7 host-agnostic.
///
/// Layout:
///
/// ```text
/// <name>.AppDir/
///   AppRun                — entry-point shell script (chmod +x on Unix)
///   <name>.desktop        — XDG desktop entry
///   usr/bin/<name>        — runtime binary (host-only) OR <name>.twebundle
/// ```
///
/// On a Linux host we copy the running `twec` binary into
/// `usr/bin/<name>` and append the bundle (mirrors the Windows
/// path). On any other host we drop a `.twebundle` next to a stub
/// README — packaging into a real AppImage requires `appimagetool`
/// (off-tree) and a Linux-native Twe runtime.
fn build_linux_appdir(
    project: &DiscoveredProject,
    out_path: &Path,
    args: &BuildArgs,
    resolved: &ResolvedConfig,
) -> i32 {
    let appdir = if out_path.extension().and_then(|s| s.to_str()) == Some("AppDir") {
        out_path.to_path_buf()
    } else {
        let mut p = out_path.to_path_buf();
        p.set_extension("AppDir");
        p
    };
    if let Err(e) = fs::create_dir_all(appdir.join("usr/bin")) {
        eprintln!("error: cannot create '{}': {e}", appdir.display());
        return 1;
    }
    let exec_name = project.name.clone();
    let desktop_path = appdir.join(format!("{exec_name}.desktop"));
    if let Err(e) = fs::write(&desktop_path, render_desktop_entry(&exec_name)) {
        eprintln!("error: cannot write '{}': {e}", desktop_path.display());
        return 1;
    }
    let apprun_path = appdir.join("AppRun");
    if let Err(e) = fs::write(&apprun_path, render_apprun_script(&exec_name)) {
        eprintln!("error: cannot write '{}': {e}", apprun_path.display());
        return 1;
    }
    // `AppRun` must be executable for AppImage runtime to invoke it.
    // On non-Unix hosts we still write the file — the contributor is
    // expected to package on Linux later, where chmod is available.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mut perms) = fs::metadata(&apprun_path).map(|m| m.permissions()) {
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&apprun_path, perms);
        }
    }
    let bundle_bytes =
        match encode_bundle_to_vec(project, resolved.compress, args.target, args.config) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
    let exec_path = appdir.join("usr/bin").join(&exec_name);
    let host_is_linux = BuildTarget::host() == BuildTarget::LinuxX86_64;
    if host_is_linux {
        let runtime = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: cannot locate twec runtime binary: {e}");
                return 1;
            }
        };
        match crate::bundle::append_to_binary(&runtime, &bundle_bytes, &exec_path) {
            Ok(offset) => {
                eprintln!(
                    "[twec build] wrote {} (runtime {} bytes + bundle {} bytes; \
                     run `appimagetool {}` on a Linux host to package)",
                    appdir.display(),
                    offset,
                    bundle_bytes.len(),
                    appdir.display()
                );
                0
            }
            Err(e) => {
                eprintln!("error: appending bundle to runtime: {e}");
                1
            }
        }
    } else {
        let bundle_path = appdir
            .join("usr/bin")
            .join(format!("{exec_name}.twebundle"));
        if let Err(e) = fs::write(&bundle_path, &bundle_bytes) {
            eprintln!("error: cannot write '{}': {e}", bundle_path.display());
            return 1;
        }
        let readme = appdir.join("usr/bin/README.txt");
        let body = format!(
            "This AppDir was produced on a non-Linux host ({}). \
             usr/bin/{exec_name} is missing — produce one by running \
             `twec build --target linux-x86_64` on a Linux host, or \
             ship the bundle through cargo-dist's release machinery.\n",
            BuildTarget::host().label()
        );
        let _ = fs::write(&readme, body);
        eprintln!(
            "[twec build] wrote {} ({} bundle bytes; cross-compile — \
             host '{}' cannot package an ELF runtime yet)",
            appdir.display(),
            bundle_bytes.len(),
            BuildTarget::host().label()
        );
        0
    }
}

/// XDG `.desktop` entry for the AppDir. Minimal — `Type`, `Name`,
/// `Exec`, `Icon`, `Categories`. `appimagetool` requires `Categories`
/// to be non-empty.
pub fn render_desktop_entry(exec_name: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={exec_name}\n\
         Exec={exec_name}\n\
         Icon={exec_name}\n\
         Categories=Game;\n\
         Terminal=false\n"
    )
}

/// `AppRun` shell script. Resolves the AppDir's location at runtime
/// via `readlink -f` so the produced binary is relocatable, then
/// `exec`s the real binary under `usr/bin/`. Forwards all arguments
/// (`"$@"`) so launcher integrations can pass flags through.
pub fn render_apprun_script(exec_name: &str) -> String {
    format!(
        "#!/bin/sh\n\
         HERE=$(dirname \"$(readlink -f \"$0\")\")\n\
         exec \"$HERE/usr/bin/{exec_name}\" \"$@\"\n"
    )
}

/// Reverse-DNS-friendly identifier slug. Strip anything not in
/// `[a-zA-Z0-9-]` (CFBundleIdentifier's character set) and lower-
/// case the result.
fn sanitize_identifier(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if s.is_empty() {
        "twe-game".to_string()
    } else {
        s
    }
}

fn resolve_out_path(project: &DiscoveredProject, args: &BuildArgs) -> Result<PathBuf, String> {
    if let Some(p) = &args.out {
        return Ok(p.clone());
    }
    let mut name = project.name.clone();
    name.push_str(args.target.binary_extension());
    Ok(project.root.join("dist").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_parses_aliases() {
        assert_eq!(
            BuildTarget::parse("windows-x86_64"),
            Some(BuildTarget::WindowsX86_64)
        );
        assert_eq!(
            BuildTarget::parse("x86_64-pc-windows-msvc"),
            Some(BuildTarget::WindowsX86_64)
        );
        assert_eq!(
            BuildTarget::parse("macos-aarch64"),
            Some(BuildTarget::MacOsAarch64)
        );
        assert_eq!(BuildTarget::parse("nonsense"), None);
    }

    #[test]
    fn config_parses() {
        assert_eq!(BuildConfig::parse("dev"), Some(BuildConfig::Dev));
        assert_eq!(BuildConfig::parse("release"), Some(BuildConfig::Release));
        assert_eq!(BuildConfig::parse("profile"), Some(BuildConfig::Profile));
        assert_eq!(BuildConfig::parse("optimized"), None);
    }

    #[test]
    fn target_extensions() {
        assert_eq!(BuildTarget::WindowsX86_64.binary_extension(), ".exe");
        assert_eq!(BuildTarget::LinuxX86_64.binary_extension(), "");
        assert_eq!(BuildTarget::MacOsAarch64.binary_extension(), "");
    }
}
