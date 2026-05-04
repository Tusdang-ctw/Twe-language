// Phase 12 sessions 1+: `twec build` orchestration tests.
//
// We stand up tiny in-tree project trees inside the `target/` test
// staging area and exercise discover / validate / dry-run end-to-end.
// Tests that need real artifacts (sessions 4+) live alongside; this
// file holds the non-IO-heavy ones.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use twec::build::{
    discover_project, parse_manifest, resolve_config, validate_project, write_bundle, BuildConfig,
    BuildTarget,
};
use twec::bundle::{
    append_to_binary, clear_active_bundle, detect_in_file, has_active_bundle, read_asset_bytes,
    set_active_bundle, BundleReader,
};

/// Serializes tests that mutate the process-global `ACTIVE_BUNDLE`
/// slot. Cargo runs tests in parallel by default; without this
/// guard two tests installing different bundles would race.
static BUNDLE_TEST_LOCK: Mutex<()> = Mutex::new(());

fn temp_project(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("twec_build_{}_{}", name, std::process::id()));
    if base.exists() {
        let _ = fs::remove_dir_all(&base);
    }
    fs::create_dir_all(&base).expect("create temp project");
    base
}

#[test]
fn discover_finds_main_and_walks_assets() {
    let dir = temp_project("discover_basic");
    fs::write(dir.join("main.twe"), "print(\"hi\")\n").unwrap();
    fs::create_dir_all(dir.join("assets/sprites")).unwrap();
    fs::write(dir.join("assets/walk.png"), b"\x89PNG fake").unwrap();
    fs::write(dir.join("assets/sprites/hero.png"), b"\x89PNG fake").unwrap();
    fs::write(dir.join("assets/audio.ogg"), b"OggS fake").unwrap();

    let project = discover_project(&dir).expect("discover");
    assert_eq!(project.name, dir.file_name().unwrap().to_string_lossy());
    assert_eq!(project.assets.len(), 3);
    let keys: Vec<_> = project.assets.iter().map(|a| a.bundle_key.as_str()).collect();
    // Sorted, forward-slash-canonical bundle keys.
    assert_eq!(
        keys,
        vec!["assets/audio.ogg", "assets/sprites/hero.png", "assets/walk.png"]
    );
    assert!(project.manifest.is_none());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_picks_up_optional_manifest() {
    let dir = temp_project("discover_manifest");
    fs::write(dir.join("main.twe"), "print(\"hi\")\n").unwrap();
    fs::write(dir.join("twe.toml"), "[project]\nname=\"x\"\n").unwrap();
    let project = discover_project(&dir).expect("discover");
    assert!(project.manifest.is_some());
    assert!(project.assets.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_errors_when_main_missing() {
    let dir = temp_project("discover_missing");
    let err = discover_project(&dir).expect_err("should fail without main.twe");
    assert!(err.contains("main.twe"), "got: {err}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_errors_when_path_not_a_directory() {
    let dir = temp_project("discover_not_a_dir");
    let file = dir.join("main.twe");
    fs::write(&file, "print(\"hi\")\n").unwrap();
    let err = discover_project(&file).expect_err("should fail when given a file");
    assert!(err.contains("not a directory") || err.contains("missing"), "got: {err}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn validate_succeeds_on_well_formed_program() {
    let dir = temp_project("validate_ok");
    fs::write(
        dir.join("main.twe"),
        "let x = 1\nlet y = x + 2\nprint(y)\n",
    )
    .unwrap();
    let project = discover_project(&dir).expect("discover");
    validate_project(&project).expect("well-formed program should validate");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn validate_reports_parse_error_with_path() {
    let dir = temp_project("validate_parse_err");
    // Unterminated string is a hard lex / parse failure.
    fs::write(dir.join("main.twe"), "let x = \"oops\nlet y = x\n").unwrap();
    let project = discover_project(&dir).expect("discover");
    let err = validate_project(&project).expect_err("should fail");
    assert!(
        err.contains("main.twe"),
        "error should reference main.twe: {err}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn target_host_returns_a_real_target() {
    // `BuildTarget::host` must always produce a valid target so the
    // CLI default works on any contributor's machine.
    let host = BuildTarget::host();
    let label = host.label();
    assert!(BuildTarget::parse(label).is_some(), "host label round-trips");
}

#[test]
fn config_round_trips() {
    for c in [BuildConfig::Dev, BuildConfig::Release, BuildConfig::Profile] {
        assert_eq!(BuildConfig::parse(c.label()), Some(c));
    }
}

#[test]
fn write_bundle_round_trips_main_and_assets() {
    let dir = temp_project("write_bundle");
    fs::write(dir.join("main.twe"), "print(\"hi\")\n").unwrap();
    fs::create_dir_all(dir.join("assets")).unwrap();
    fs::write(dir.join("assets/walk.png"), b"\x89PNG demo").unwrap();
    fs::write(dir.join("assets/audio.ogg"), b"OggS demo").unwrap();
    let project = discover_project(&dir).expect("discover");
    let bundle_out = dir.join("out.twebundle");
    let bytes = write_bundle(&project, &bundle_out).expect("write");
    assert!(bytes > 0);
    assert!(bundle_out.is_file());
    let mut reader = BundleReader::open(&bundle_out).expect("open");
    assert_eq!(reader.entry_count(), 3);
    assert!(reader.has("main.twe"));
    assert!(reader.has("assets/walk.png"));
    assert!(reader.has("assets/audio.ogg"));
    let main = reader.read("main.twe").unwrap().expect("present");
    assert_eq!(main, b"print(\"hi\")\n");
    let walk = reader.read("assets/walk.png").unwrap().expect("present");
    assert_eq!(walk, b"\x89PNG demo");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn write_bundle_creates_missing_parent_dirs() {
    let dir = temp_project("write_bundle_parents");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    let project = discover_project(&dir).expect("discover");
    let nested_out = dir.join("does/not/exist/out.twebundle");
    write_bundle(&project, &nested_out).expect("write");
    assert!(nested_out.is_file());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn active_bundle_redirects_then_falls_through_to_filesystem() {
    // Phase 12 session 3: install a bundle, observe `read_asset_bytes`
    // returning bundle bodies; clear the bundle, observe the same
    // call falling through to the filesystem; verify a path that's
    // in neither errors with NotFound.
    let _guard = BUNDLE_TEST_LOCK.lock().expect("test lock poisoned");
    clear_active_bundle();

    let dir = temp_project("active_bundle");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    fs::create_dir_all(dir.join("assets")).unwrap();
    fs::write(dir.join("assets/sprite.png"), b"BUNDLE_VERSION").unwrap();
    let project = discover_project(&dir).expect("discover");
    let bundle_path = dir.join("out.twebundle");
    write_bundle(&project, &bundle_path).expect("write");
    let reader = BundleReader::open(&bundle_path).expect("open");
    set_active_bundle(reader);
    assert!(has_active_bundle());

    // Bundle hit: returns the embedded bytes verbatim.
    let bytes = read_asset_bytes("assets/sprite.png").expect("bundle hit");
    assert_eq!(bytes, b"BUNDLE_VERSION");

    // Bundle miss → filesystem fallback. We write a different
    // version on disk to prove the path actually resolves to the
    // installed bundle when the key matches.
    fs::write(dir.join("assets/sprite.png"), b"DISK_VERSION").unwrap();
    let bytes_after = read_asset_bytes("assets/sprite.png").expect("still hits bundle");
    assert_eq!(bytes_after, b"BUNDLE_VERSION", "bundle wins over disk");

    // Now clear: same call falls through to disk.
    clear_active_bundle();
    assert!(!has_active_bundle());
    let bytes_disk = read_asset_bytes(dir.join("assets/sprite.png").to_str().unwrap())
        .expect("filesystem fallback");
    assert_eq!(bytes_disk, b"DISK_VERSION");

    // Missing path errors with NotFound.
    let err = read_asset_bytes("does_not_exist_anywhere.png")
        .expect_err("missing path should error");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn manifest_parses_project_name_and_defaults() {
    let dir = temp_project("manifest_parse");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    fs::write(
        dir.join("twe.toml"),
        r#"
[project]
name = "survive"

[build]
default_target = "windows-x86_64"
default_config = "release"

[build.dev]
hot_reload = true
bundle_assets = false

[build.release]
hot_reload = false
bundle_assets = true
strip_debug = true

[build.profile]
profile = true
"#,
    )
    .unwrap();
    let manifest = parse_manifest(&dir.join("twe.toml")).expect("parse");
    assert_eq!(manifest.project_name.as_deref(), Some("survive"));
    assert_eq!(manifest.default_target, Some(BuildTarget::WindowsX86_64));
    assert_eq!(manifest.default_config, Some(BuildConfig::Release));
    assert_eq!(manifest.configs.len(), 3);
    assert_eq!(
        manifest.configs.get("dev").unwrap().hot_reload,
        Some(true)
    );
    assert_eq!(
        manifest.configs.get("profile").unwrap().profile,
        Some(true)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn manifest_parse_rejects_unknown_target_string() {
    let dir = temp_project("manifest_bad_target");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    fs::write(
        dir.join("twe.toml"),
        "[build]\ndefault_target = \"nope-x86_64\"\n",
    )
    .unwrap();
    let err = parse_manifest(&dir.join("twe.toml")).expect_err("unknown target");
    assert!(err.contains("default_target"), "got: {err}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn resolve_config_applies_manifest_overrides() {
    use twec::build::ConfigOverride;
    let mut manifest = twec::build::ProjectManifest::default();
    manifest.configs.insert(
        "release".to_string(),
        ConfigOverride {
            hot_reload: Some(true),  // override the builtin false
            bundle_assets: None,
            strip_debug: Some(false),
            profile: None,
        },
    );
    let resolved = resolve_config(Some(&manifest), BuildConfig::Release);
    assert!(resolved.hot_reload, "manifest override applied");
    // bundle_assets not overridden — should keep release default (true).
    assert!(resolved.bundle_assets);
    assert!(!resolved.strip_debug);
    // profile not overridden — keeps release default false.
    assert!(!resolved.profile);
}

#[test]
fn resolve_config_uses_builtin_defaults_without_manifest() {
    let dev = resolve_config(None, BuildConfig::Dev);
    assert!(dev.hot_reload);
    assert!(!dev.bundle_assets);

    let release = resolve_config(None, BuildConfig::Release);
    assert!(!release.hot_reload);
    assert!(release.bundle_assets);
    assert!(release.strip_debug);
    assert!(!release.profile);

    let profile = resolve_config(None, BuildConfig::Profile);
    assert!(profile.bundle_assets);
    assert!(profile.profile);
}

#[test]
fn manifest_ignores_unknown_keys() {
    // Forward-compat: an old twec running against a newer project's
    // manifest should not blow up on keys it doesn't recognize.
    let dir = temp_project("manifest_unknown");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    fs::write(
        dir.join("twe.toml"),
        r#"
[project]
name = "x"
some_future_key = 42

[build]
default_config = "release"

[build.release]
hot_reload = false
some_future_flag = "yes please"
"#,
    )
    .unwrap();
    parse_manifest(&dir.join("twe.toml")).expect("should not fail on unknown keys");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn append_to_binary_round_trips_via_detect_in_file() {
    // Phase 12 session 4: simulate a self-extracting `.exe` by
    // appending a bundle to a fake runtime binary, then probe it
    // back via `detect_in_file`. Verifies (a) the boot footer is
    // written correctly, (b) `detect_in_file` returns Some(reader),
    // (c) the recovered bundle hands back the original bytes.
    let dir = temp_project("append_to_binary");
    let runtime_path = dir.join("fake_runtime.exe");
    // The "runtime" is just opaque bytes; nothing reads it as a PE
    // because `detect_in_file` only inspects the trailing footer.
    let runtime_bytes: Vec<u8> = (0u8..200u8).collect();
    fs::write(&runtime_path, &runtime_bytes).unwrap();
    fs::write(dir.join("main.twe"), "print(\"embedded\")\n").unwrap();
    fs::create_dir_all(dir.join("assets")).unwrap();
    fs::write(dir.join("assets/a.txt"), b"alpha").unwrap();
    let project = discover_project(&dir).expect("discover");

    let mut bundle_buf = Vec::new();
    let main_bytes = fs::read(&project.main).unwrap();
    let asset_bytes = fs::read(&project.assets[0].abs).unwrap();
    twec::bundle::encode(
        &mut bundle_buf,
        &[
            ("main.twe".to_string(), main_bytes.clone()),
            (project.assets[0].bundle_key.clone(), asset_bytes.clone()),
        ],
    )
    .unwrap();

    let out = dir.join("hosted.exe");
    let bundle_offset = append_to_binary(&runtime_path, &bundle_buf, &out).unwrap();
    assert_eq!(bundle_offset, runtime_bytes.len() as u64);

    let detected = detect_in_file(&out).unwrap();
    let mut reader = detected.expect("footer should be detected");
    assert_eq!(reader.entry_count(), 2);
    let main_back = reader.read("main.twe").unwrap().expect("main present");
    assert_eq!(main_back, main_bytes);
    let asset_back = reader
        .read(&project.assets[0].bundle_key)
        .unwrap()
        .expect("asset present");
    assert_eq!(asset_back, asset_bytes);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn detect_in_file_returns_none_for_non_extracting_binary() {
    // A file with no boot footer should produce Ok(None), not an
    // error — the production path runs this check on every
    // `twec.exe` startup, so a plain CLI binary must succeed.
    let dir = temp_project("no_footer");
    let plain = dir.join("no_footer.bin");
    let bytes: Vec<u8> = (0u8..150u8).collect();
    fs::write(&plain, bytes).unwrap();
    let result = detect_in_file(&plain).unwrap();
    assert!(result.is_none());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn read_asset_bytes_falls_through_when_no_bundle_set() {
    // Same fallthrough path as the cleared half of the previous
    // test, run independently to avoid leaking state if that test
    // races / aborts.
    let _guard = BUNDLE_TEST_LOCK.lock().expect("test lock poisoned");
    clear_active_bundle();
    let dir = temp_project("fall_through");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    fs::write(dir.join("hello.txt"), b"FROM_DISK").unwrap();
    let bytes = read_asset_bytes(dir.join("hello.txt").to_str().unwrap())
        .expect("filesystem read");
    assert_eq!(bytes, b"FROM_DISK");
    let _ = fs::remove_dir_all(&dir);
}
