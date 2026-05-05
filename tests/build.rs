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
    discover_project, encode_bundle_to_vec, parse_manifest, render_app_build_vdf,
    render_apprun_script, render_depot_build_vdf, render_desktop_entry, render_info_plist,
    resolve_config, validate_project, write_bundle, write_bundle_with_options,
    write_steam_layout, BuildArgs, BuildConfig, BuildTarget,
};
use twec::bundle::{
    append_to_binary, clear_active_bundle, detect_in_file, encode_with_options, has_active_bundle,
    read_asset_bytes, set_active_bundle, BundleProvenance, BundleReader, EncodeOptions, FLAG_ZSTD,
    PROVENANCE_KEY,
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
            compress: None,
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

// ---------- Phase 12 session 6: macOS .app skeleton ----------

#[test]
fn info_plist_contains_required_bundle_keys() {
    let plist = render_info_plist("survive", "0.1.0-pre");
    for key in [
        "CFBundleExecutable",
        "CFBundleIdentifier",
        "CFBundleName",
        "CFBundlePackageType",
        "CFBundleShortVersionString",
        "CFBundleVersion",
    ] {
        assert!(plist.contains(key), "plist missing {key}: {plist}");
    }
    assert!(plist.contains("<string>survive</string>"));
    assert!(plist.contains("<string>APPL</string>"));
    assert!(plist.contains("<string>0.1.0-pre</string>"));
}

#[test]
fn info_plist_sanitizes_identifier() {
    // Spaces / underscores / non-ASCII chars must come out as `-`
    // because CFBundleIdentifier rejects them.
    let plist = render_info_plist("My Game_v2 (beta)", "1.0");
    assert!(
        plist.contains("<string>dev.twe.my-game-v2--beta-</string>"),
        "got: {plist}"
    );
}

// ---------- Phase 12 session 7: Linux AppDir layout ----------

#[test]
fn desktop_entry_has_required_xdg_keys() {
    let entry = render_desktop_entry("survive");
    for key in ["[Desktop Entry]", "Type=", "Name=", "Exec=", "Categories="] {
        assert!(entry.contains(key), "entry missing {key}: {entry}");
    }
    assert!(entry.contains("Name=survive"));
    assert!(entry.contains("Exec=survive"));
}

#[test]
fn apprun_script_resolves_relative_to_self() {
    let script = render_apprun_script("survive");
    assert!(script.starts_with("#!/bin/sh\n"), "got: {script}");
    assert!(script.contains("readlink -f"), "must resolve symlinks");
    assert!(
        script.contains("usr/bin/survive"),
        "must exec the AppDir binary"
    );
    assert!(script.contains("\"$@\""), "must forward arguments");
}

// ---------- Phase 12 session 8: zstd bundle compression ----------

#[test]
fn compressed_bundle_round_trips() {
    let dir = temp_project("zstd_round_trip");
    let bundle_path = dir.join("compressed.twebundle");
    let body: Vec<u8> = std::iter::repeat_n(b'A', 16 * 1024).collect();
    let entries = vec![
        ("main.twe".to_string(), b"print(1)\n".to_vec()),
        ("assets/big.txt".to_string(), body.clone()),
    ];
    let mut compressed_buf = Vec::new();
    encode_with_options(
        &mut compressed_buf,
        &entries,
        EncodeOptions { compress: true },
    )
    .unwrap();
    let mut raw_buf = Vec::new();
    encode_with_options(&mut raw_buf, &entries, EncodeOptions::default()).unwrap();
    assert!(
        compressed_buf.len() < raw_buf.len() / 2,
        "compressed ({}) should be far smaller than raw ({})",
        compressed_buf.len(),
        raw_buf.len()
    );
    fs::write(&bundle_path, &compressed_buf).unwrap();
    let mut reader = BundleReader::open(&bundle_path).expect("open");
    assert_eq!(reader.header.flags & FLAG_ZSTD, FLAG_ZSTD);
    let main = reader.read("main.twe").unwrap().expect("present");
    assert_eq!(main, b"print(1)\n");
    let big = reader.read("assets/big.txt").unwrap().expect("present");
    assert_eq!(big, body, "decompressed body mismatches input");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn write_bundle_with_options_compress_flag_round_trips_through_reader() {
    let dir = temp_project("zstd_pipeline");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    fs::create_dir_all(dir.join("assets")).unwrap();
    let asset_body: Vec<u8> = std::iter::repeat_n(b'X', 8 * 1024).collect();
    fs::write(dir.join("assets/blob.bin"), &asset_body).unwrap();
    let project = discover_project(&dir).expect("discover");
    let out = dir.join("out.twebundle");
    write_bundle_with_options(&project, &out, EncodeOptions { compress: true })
        .expect("write compressed");
    let mut reader = BundleReader::open(&out).expect("open");
    assert_eq!(reader.header.flags & FLAG_ZSTD, FLAG_ZSTD);
    assert_eq!(
        reader.read("assets/blob.bin").unwrap().expect("present"),
        asset_body
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn release_config_default_compresses() {
    let release = resolve_config(None, BuildConfig::Release);
    assert!(release.compress, "release should compress by default");
    let dev = resolve_config(None, BuildConfig::Dev);
    assert!(!dev.compress, "dev should skip compression");
    let profile = resolve_config(None, BuildConfig::Profile);
    assert!(profile.compress, "profile should compress");
}

#[test]
fn manifest_compress_override_round_trips() {
    let dir = temp_project("compress_override");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    fs::write(
        dir.join("twe.toml"),
        "[build.release]\ncompress = false\n",
    )
    .unwrap();
    let manifest = parse_manifest(&dir.join("twe.toml")).expect("parse");
    let resolved = resolve_config(Some(&manifest), BuildConfig::Release);
    assert!(!resolved.compress, "manifest override should disable compression");
    assert!(resolved.bundle_assets);
    let _ = fs::remove_dir_all(&dir);
}

// ---------- Phase 12 session 9: Steam Depot layout ----------

#[test]
fn app_build_vdf_has_required_keys() {
    let vdf = render_app_build_vdf(480, 481, "test build");
    assert!(vdf.contains("\"appbuild\""));
    assert!(vdf.contains("\"appid\" \"480\""));
    assert!(vdf.contains("\"depots\""));
    assert!(vdf.contains("\"481\" \"depot_build_481.vdf\""));
    assert!(vdf.contains("\"contentroot\" \"content\""));
}

#[test]
fn depot_build_vdf_has_required_keys() {
    let vdf = render_depot_build_vdf(481);
    assert!(vdf.contains("\"DepotBuildConfig\""));
    assert!(vdf.contains("\"DepotID\" \"481\""));
    assert!(vdf.contains("\"FileMapping\""));
    assert!(vdf.contains("\"recursive\" \"1\""));
}

#[test]
fn write_steam_layout_creates_expected_files() {
    let dir = temp_project("steam_layout");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    let project = discover_project(&dir).expect("discover");
    let out_path = dir.join("dist").join(&project.name);
    fs::create_dir_all(out_path.parent().unwrap()).unwrap();
    let args = BuildArgs {
        project_dir: dir.clone(),
        target: BuildTarget::host(),
        target_explicit: false,
        config: BuildConfig::Release,
        config_explicit: false,
        out: None,
        dry_run: false,
        steam: true,
    };
    write_steam_layout(&project, &out_path, None, &args).expect("layout");
    let steam_dir = out_path
        .parent()
        .unwrap()
        .join(format!("{}.steam", project.name));
    assert!(steam_dir.is_dir());
    assert!(steam_dir.join("steam_appid.txt").is_file());
    assert!(steam_dir.join("content").is_dir());
    assert!(steam_dir.join("README.txt").is_file());
    let appid = fs::read_to_string(steam_dir.join("steam_appid.txt")).unwrap();
    assert_eq!(appid.trim(), "480");
    assert!(steam_dir.join("app_build_480.vdf").is_file());
    assert!(steam_dir.join("depot_build_481.vdf").is_file());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn steam_manifest_overrides_app_id() {
    let dir = temp_project("steam_manifest");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    fs::write(
        dir.join("twe.toml"),
        "[steam]\nenabled = true\napp_id = 1234560\ndepot_id = 1234561\ndepot_description = \"survive v0.6\"\n",
    )
    .unwrap();
    let manifest = parse_manifest(&dir.join("twe.toml")).expect("parse");
    let steam = manifest.steam.as_ref().expect("steam manifest parsed");
    assert!(steam.enabled);
    assert_eq!(steam.app_id, Some(1234560));
    assert_eq!(steam.depot_id, Some(1234561));
    assert_eq!(steam.depot_description.as_deref(), Some("survive v0.6"));

    let project = discover_project(&dir).expect("discover");
    let out_path = dir.join("dist").join(&project.name);
    fs::create_dir_all(out_path.parent().unwrap()).unwrap();
    let args = BuildArgs {
        project_dir: dir.clone(),
        target: BuildTarget::host(),
        target_explicit: false,
        config: BuildConfig::Release,
        config_explicit: false,
        out: None,
        dry_run: false,
        steam: true,
    };
    write_steam_layout(&project, &out_path, Some(&manifest), &args).expect("layout");
    let steam_dir = out_path
        .parent()
        .unwrap()
        .join(format!("{}.steam", project.name));
    let appid = fs::read_to_string(steam_dir.join("steam_appid.txt")).unwrap();
    assert_eq!(appid.trim(), "1234560");
    assert!(steam_dir.join("app_build_1234560.vdf").is_file());
    let app_build = fs::read_to_string(steam_dir.join("app_build_1234560.vdf")).unwrap();
    assert!(app_build.contains("\"1234561\" \"depot_build_1234561.vdf\""));
    assert!(app_build.contains("survive v0.6"));
    let _ = fs::remove_dir_all(&dir);
}

// ---------- Phase 12 session 10: build provenance + twec info ----------

#[test]
fn provenance_round_trips_through_toml() {
    let original = BundleProvenance {
        twec_version: "0.1.0-pre".to_string(),
        host_os: "windows".to_string(),
        host_arch: "x86_64".to_string(),
        build_unix_secs: 1_700_000_000,
        project_name: "survive_demo".to_string(),
        target: "windows-x86_64".to_string(),
        config: "release".to_string(),
        compress: true,
        entry_count: 42,
    };
    let toml = original.to_toml();
    let parsed = BundleProvenance::from_toml(&toml).expect("parse");
    assert_eq!(original, parsed);
}

#[test]
fn provenance_from_toml_rejects_missing_keys() {
    // Drop the `twec_version` line — should fail with a key-named error.
    let bad = "[provenance]\n\
               host_os = \"linux\"\n\
               host_arch = \"x86_64\"\n\
               build_unix_secs = 1\n\
               project_name = \"x\"\n\
               target = \"linux-x86_64\"\n\
               config = \"dev\"\n\
               compress = false\n\
               entry_count = 0\n";
    let err = BundleProvenance::from_toml(bad).expect_err("missing key");
    assert!(err.contains("twec_version"), "got: {err}");
}

#[test]
fn provenance_escapes_quotes_in_project_name() {
    let p = BundleProvenance {
        twec_version: "0".to_string(),
        host_os: "linux".to_string(),
        host_arch: "x86_64".to_string(),
        build_unix_secs: 0,
        project_name: "weird \"quoted\" name".to_string(),
        target: "linux-x86_64".to_string(),
        config: "dev".to_string(),
        compress: false,
        entry_count: 0,
    };
    let toml = p.to_toml();
    let parsed = BundleProvenance::from_toml(&toml).expect("round-trip");
    assert_eq!(parsed.project_name, "weird \"quoted\" name");
}

#[test]
fn build_pipeline_writes_provenance_entry() {
    let dir = temp_project("prov_pipeline");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    fs::create_dir_all(dir.join("assets")).unwrap();
    fs::write(dir.join("assets/sprite.png"), b"PNGFAKE").unwrap();
    let project = discover_project(&dir).expect("discover");
    let bundle_bytes = encode_bundle_to_vec(
        &project,
        false,
        BuildTarget::WindowsX86_64,
        BuildConfig::Release,
    )
    .expect("encode");
    let bundle_path = dir.join("p.twebundle");
    fs::write(&bundle_path, &bundle_bytes).unwrap();
    let mut reader = BundleReader::open(&bundle_path).expect("open");
    let prov_bytes = reader
        .read(PROVENANCE_KEY)
        .unwrap()
        .expect("provenance entry present");
    let toml = std::str::from_utf8(&prov_bytes).expect("utf8");
    let p = BundleProvenance::from_toml(toml).expect("parse");
    assert_eq!(p.target, "windows-x86_64");
    assert_eq!(p.config, "release");
    assert!(!p.compress);
    // 2 user-facing entries (main.twe + assets/sprite.png).
    assert_eq!(p.entry_count, 2);
    assert_eq!(p.project_name, project.name);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_info_succeeds_on_built_bundle() {
    let dir = temp_project("info_smoke");
    fs::write(dir.join("main.twe"), "print(1)\n").unwrap();
    let project = discover_project(&dir).expect("discover");
    let bundle_bytes = encode_bundle_to_vec(
        &project,
        true,
        BuildTarget::LinuxX86_64,
        BuildConfig::Profile,
    )
    .expect("encode");
    let bundle_path = dir.join("info.twebundle");
    fs::write(&bundle_path, &bundle_bytes).unwrap();
    let exit = twec::build::run_info(&bundle_path);
    assert_eq!(exit, 0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_info_errors_on_non_bundle_file() {
    let dir = temp_project("info_garbage");
    let path = dir.join("not_a_bundle.bin");
    fs::write(&path, b"this is not a bundle").unwrap();
    let exit = twec::build::run_info(&path);
    assert_ne!(exit, 0, "info should fail on garbage input");
    let _ = fs::remove_dir_all(&dir);
}

// ---------- Phase 12 session 11: EXIT GATE — survive_demo end-to-end ----------

#[test]
fn survive_demo_project_validates() {
    // The EXIT GATE deliverable. `examples/survive_demo/` is a real
    // on-disk project tree (`main.twe` + `twe.toml` + `assets/`). The
    // discovery + validation path is the same pipeline `twec build`
    // runs before producing an artifact, so this test catches
    // regressions that would prevent the demo shipping.
    let demo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/survive_demo");
    assert!(demo.is_dir(), "survive_demo project missing at {}", demo.display());
    assert!(demo.join("main.twe").is_file());
    assert!(demo.join("twe.toml").is_file());
    assert!(demo.join("assets/hero.png").is_file());

    let project = discover_project(&demo).expect("discover");
    assert_eq!(project.name, "survive_demo");
    assert!(
        project.assets.iter().any(|a| a.bundle_key == "assets/hero.png"),
        "hero asset must be picked up"
    );
    let manifest_path = project.manifest.as_ref().expect("manifest").clone();
    let manifest = parse_manifest(&manifest_path).expect("parse manifest");
    assert_eq!(manifest.default_config, Some(BuildConfig::Release));
    let steam = manifest.steam.as_ref().expect("steam manifest set");
    assert!(steam.enabled);
    validate_project(&project).expect("survive_demo main.twe must validate");
}

#[test]
fn survive_demo_round_trips_through_bundle_pipeline() {
    // End-to-end: encode the demo project's bundle, write it to a
    // tempfile, read it back through the public `twec info` path
    // (`run_info`), and confirm the provenance + entry layout match
    // what a Steam-class build would deliver.
    let demo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/survive_demo");
    let project = discover_project(&demo).expect("discover");
    let bytes = encode_bundle_to_vec(
        &project,
        true,
        BuildTarget::WindowsX86_64,
        BuildConfig::Release,
    )
    .expect("encode");
    let out_dir = std::env::temp_dir().join(format!(
        "twec_survive_demo_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&out_dir).unwrap();
    let bundle_path = out_dir.join("survive_demo.twebundle");
    fs::write(&bundle_path, &bytes).unwrap();

    let mut reader = BundleReader::open(&bundle_path).expect("open");
    assert!(reader.has("main.twe"));
    assert!(reader.has("assets/hero.png"));
    assert!(reader.has(PROVENANCE_KEY));
    assert_eq!(reader.header.flags & FLAG_ZSTD, FLAG_ZSTD);

    let prov_bytes = reader.read(PROVENANCE_KEY).unwrap().expect("present");
    let toml = std::str::from_utf8(&prov_bytes).unwrap();
    let prov = BundleProvenance::from_toml(toml).expect("parse provenance");
    assert_eq!(prov.project_name, "survive_demo");
    assert_eq!(prov.target, "windows-x86_64");
    assert_eq!(prov.config, "release");
    assert!(prov.compress);
    // 2 user-facing entries: main.twe + assets/hero.png.
    assert_eq!(prov.entry_count, 2);

    let exit = twec::build::run_info(&bundle_path);
    assert_eq!(exit, 0);
    let _ = fs::remove_dir_all(&out_dir);
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
