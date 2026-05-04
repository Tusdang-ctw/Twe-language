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
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigOverride {
    pub hot_reload: Option<bool>,
    pub bundle_assets: Option<bool>,
    pub strip_debug: Option<bool>,
    pub profile: Option<bool>,
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
            },
            BuildConfig::Release => Self {
                config,
                hot_reload: false,
                bundle_assets: true,
                strip_debug: true,
                profile: false,
            },
            BuildConfig::Profile => Self {
                config,
                hot_reload: false,
                bundle_assets: true,
                strip_debug: true,
                profile: true,
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
    }
}

/// Parse a `twe.toml` from disk. Errors carry the file path so
/// diagnostics are clickable. Unknown keys are ignored — the manifest
/// surface evolves over phases and old projects shouldn't break on
/// new keys.
pub fn parse_manifest(path: &Path) -> Result<ProjectManifest, String> {
    let display = path.display().to_string();
    let src = fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{display}': {e}"))?;
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
            };
            manifest.configs.insert(key.to_string(), ovr);
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
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("cannot read directory '{}': {e}", dir.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| format!("error walking '{}': {e}", dir.display()))?;
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
    let src = fs::read_to_string(&project.main)
        .map_err(|e| format!("cannot read '{main_str}': {e}"))?;
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
         (hot_reload={}, bundle_assets={}, strip_debug={}, profile={})",
        args.target.label(),
        args.config.label(),
        resolved.hot_reload,
        resolved.bundle_assets,
        resolved.strip_debug,
        resolved.profile,
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
    match args.target {
        BuildTarget::WindowsX86_64 if BuildTarget::host() == BuildTarget::WindowsX86_64 => {
            build_self_extracting(&project, &out_path, &args)
        }
        _ => {
            // Standalone bundle for other targets (and for cross-
            // compile from non-Windows hosts to Windows, which is
            // a session-6 / -7 follow-on).
            let bundle_out = bundle_out_path(&out_path);
            match write_bundle(&project, &bundle_out) {
                Ok(bytes_written) => {
                    eprintln!(
                        "[twec build] wrote bundle: {} ({} bytes, {} entries)",
                        bundle_out.display(),
                        bytes_written,
                        project.assets.len() + 1
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
    }
}

/// Phase 12 session 4: produce a single self-extracting `.exe` by
/// (a) encoding the bundle to memory, (b) copying the running
/// `twec.exe` to the output path, (c) appending the bundle bytes +
/// footer. The runtime detects the footer at startup via
/// `bundle::detect_in_self`.
fn build_self_extracting(
    project: &DiscoveredProject,
    out_path: &Path,
    _args: &BuildArgs,
) -> i32 {
    // Encode the bundle to memory (small enough — full survive.twe
    // tree is ~1MB; v0.6 doesn't ship multi-GB games).
    let bundle_bytes = match encode_bundle_to_vec(project) {
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

fn encode_bundle_to_vec(project: &DiscoveredProject) -> Result<Vec<u8>, String> {
    let main_bytes = fs::read(&project.main)
        .map_err(|e| format!("cannot read '{}': {e}", project.main.display()))?;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(project.assets.len() + 1);
    entries.push(("main.twe".to_string(), main_bytes));
    for asset in &project.assets {
        let bytes = fs::read(&asset.abs)
            .map_err(|e| format!("cannot read '{}': {e}", asset.abs.display()))?;
        entries.push((asset.bundle_key.clone(), bytes));
    }
    let mut buf = Vec::new();
    crate::bundle::encode(&mut buf, &entries).map_err(|e| format!("encoding bundle: {e}"))?;
    Ok(buf)
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
/// the standalone `twec bundle` subcommand.
pub fn write_bundle(
    project: &DiscoveredProject,
    out_path: &Path,
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
    crate::bundle::encode(&mut file, &entries)
        .map_err(|e| format!("encoding bundle: {e}"))
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
